//! Phase 0 spike: GPUI overlay + XCap still capture on Windows.

mod caption;
mod redaction_editor;
use redaction_editor::{OutputMode, RedactionEditor};

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use app_core::CaptureRequest as CaptureJob;
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

struct ResidentController(gpui::Entity<SpikeShell>);
impl gpui::Global for ResidentController {}

#[cfg(all(test, target_os = "windows"))]
struct ControlLatencyProbe {
    started: std::time::Instant,
    frames_ms: Arc<Mutex<Vec<f64>>>,
}
#[cfg(all(test, target_os = "windows"))]
impl gpui::Global for ControlLatencyProbe {}

#[cfg(all(test, target_os = "windows"))]
#[derive(Clone)]
struct RegionLatencyProbe {
    started: std::time::Instant,
    events: Arc<Mutex<Vec<(&'static str, f64)>>>,
}
#[cfg(all(test, target_os = "windows"))]
impl gpui::Global for RegionLatencyProbe {}
#[cfg(all(test, target_os = "windows"))]
impl RegionLatencyProbe {
    fn mark(&self, stage: &'static str) {
        self.events
            .lock()
            .unwrap()
            .push((stage, self.started.elapsed().as_secs_f64() * 1000.0));
    }
}

#[derive(Clone, Copy)]
enum UiRequest {
    Control,
    Overlay(OverlayMode),
}

pub struct SpikeShell {
    control: Option<AnyWindowHandle>,
    resident: bool,
    graphics: bool,
    switching_graphics: bool,
    preview_active: bool,
    edit_selection: bool,
    focus_handle: FocusHandle,
    overlays: Vec<WindowHandle<Overlay>>,
    editor: Option<WindowHandle<RedactionEditor>>,
    editor_closing: bool,
    backend: XcapBackend,
    xcap_displays: Vec<DisplayInfo>,
    xcap_windows: Vec<WindowInfo>,
    refreshing: bool,
    diagnostics: bool,
    settings_loading: bool,
    settings_saving: bool,
    output_directory: Option<PathBuf>,
    settings_status: String,
    history_database: Option<PathBuf>,
    history_visible: bool,
    history_loading: bool,
    history_mutating: bool,
    history_action_status: String,
    history_refresh_pending: bool,
    history_has_more: bool,
    history_entries: Vec<app_core::HistoryEntry>,
    history_status: String,
    list_status: String,
    capture_status: String,
    last_capture: Option<CapturedImage>,
    last_saved: Option<PathBuf>,
    last_render: Option<Arc<gpui::RenderImage>>,
    capturing: bool,
    capture_delay: Option<gpui::Task<()>>,
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
    fn attach_ui(&mut self, request: UiRequest, cx: &mut Context<Self>) {
        if self.switching_graphics || self.quitting {
            return;
        }
        self.switching_graphics = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .update(|cx| {
                    cx.request_windowing(gpui::WindowingRequest::Windowed(
                        gpui::GraphicalEnvironment::detect(),
                    ))
                })
                .await;
            this.update(cx, |this, cx| {
                this.switching_graphics = false;
                match result {
                    Ok(()) => {
                        this.graphics = true;
                        #[cfg(all(test, target_os = "windows"))]
                        if let Some(probe) = cx.try_global::<RegionLatencyProbe>() {
                            probe.mark("graphics_ready");
                        }
                        if this.quitting {
                            this.finish_quit_if_idle(cx);
                            return;
                        }
                        match request {
                            UiRequest::Control => this.show_control(cx),
                            UiRequest::Overlay(mode) => this.open_overlays(mode, cx),
                        }
                    }
                    Err(error) => {
                        tracing::error!(%error, "could not attach graphical UI");
                        this.capture_status = format!("UI unavailable: {error}");
                        #[cfg(target_os = "windows")]
                        if this.tray.is_none() {
                            this.quitting = true;
                        }
                        this.finish_quit_if_idle(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn show_control(&mut self, cx: &mut Context<Self>) {
        if self.quitting || self.capturing || self.preview_active || self.switching_graphics {
            return;
        }
        if !self.graphics {
            self.attach_ui(UiRequest::Control, cx);
            return;
        }
        if self.control.is_some() {
            self.restore_control(cx);
            return;
        }
        let entity = cx.entity();
        let focus = self.focus_handle.clone();
        let owner = entity.downgrade();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(820.0), px(720.0)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("screenshots".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        self.switching_graphics = true;
        let completion = owner.clone();
        cx.defer(move |cx| {
            let result = cx.open_window(options, move |window, cx| {
                window.on_window_should_close(cx, move |_, cx| {
                    let owner = owner.clone();
                    cx.defer(move |cx| {
                        owner.update(cx, |this, cx| this.retire_ui(cx)).ok();
                    });
                    false
                });
                window.focus(&focus, cx);
                #[cfg(all(test, target_os = "windows"))]
                if let Some(probe) = cx.try_global::<ControlLatencyProbe>() {
                    let started = probe.started;
                    let frames = probe.frames_ms.clone();
                    window.on_next_frame(move |window, _| {
                        frames
                            .lock()
                            .unwrap()
                            .push(started.elapsed().as_secs_f64() * 1000.0);
                        window.on_next_frame(move |_, _| {
                            frames
                                .lock()
                                .unwrap()
                                .push(started.elapsed().as_secs_f64() * 1000.0);
                        });
                    });
                }
                entity
            });
            completion
                .update(cx, |this, cx| {
                    this.switching_graphics = false;
                    match result {
                        Ok(handle) => {
                            this.control = Some(handle.into());
                            this.restore_control(cx);
                        }
                        Err(error) => {
                            tracing::error!(%error, "could not open control window");
                            this.retire_ui(cx);
                        }
                    }
                    this.finish_quit_if_idle(cx);
                })
                .ok();
        });
    }

    fn retire_ui(&mut self, cx: &mut Context<Self>) {
        if self.editor.is_some() || self.editor_closing {
            self.capture_status = "Finish or cancel editing before closing this window".into();
            cx.notify();
            return;
        }
        self.cancel_capture_delay(cx);
        #[cfg(target_os = "windows")]
        if self.resident && self.tray.is_none() {
            cx.defer(close_application);
            return;
        }
        if !self.resident || self.switching_graphics || self.capturing {
            return;
        }
        self.preview_active = false;
        self.last_render = None;
        let mut windows: Vec<AnyWindowHandle> = self.overlays.drain(..).map(Into::into).collect();
        windows.extend(self.control.take());
        let owner = cx.entity().downgrade();
        self.switching_graphics = true;
        cx.defer(move |cx| {
            #[cfg(target_os = "windows")]
            let hidden: Result<Vec<_>, String> = windows
                .iter()
                .map(|handle| {
                    handle
                        .update(cx, |_, window, _| {
                            let native = native_window(window)?;
                            native
                                .request_visible(false)
                                .map_err(|error| error.to_string())?;
                            Ok(native)
                        })
                        .map_err(|error| error.to_string())
                        .and_then(|result| result)
                })
                .collect();
            cx.spawn(async move |cx| {
                #[cfg(target_os = "windows")]
                let settled = cx
                    .background_spawn(async move {
                        let hidden = hidden?;
                        for window in &hidden {
                            window
                                .wait_until_hidden()
                                .map_err(|error| error.to_string())?;
                        }
                        app_core::wait_for_desktop_compositor()
                            .map_err(|error| error.to_string())?;
                        app_core::wait_for_desktop_compositor()
                            .map_err(|error| error.to_string())?;
                        Ok::<_, String>(hidden)
                    })
                    .await;
                #[cfg(not(target_os = "windows"))]
                let settled = Ok::<(), String>(());
                if let Err(error) = settled {
                    owner
                        .update(cx, |this, cx| {
                            this.switching_graphics = false;
                            this.control = windows
                                .iter()
                                .find(|handle| handle.downcast::<SpikeShell>().is_some())
                                .copied();
                            this.overlays = windows
                                .iter()
                                .filter_map(|handle| handle.downcast::<Overlay>())
                                .collect();
                            this.capture_status = format!("Could not close UI: {error}");
                            this.restore_control(cx);
                        })
                        .ok();
                    return;
                }
                cx.update(|cx| {
                    for handle in windows {
                        handle
                            .update(cx, |_, window, _| window.remove_window())
                            .ok();
                    }
                });
                #[cfg(target_os = "windows")]
                if let Err(error) = cx
                    .background_spawn(async move {
                        for window in settled.unwrap() {
                            window
                                .wait_until_destroyed()
                                .map_err(|error| error.to_string())?;
                        }
                        Ok::<_, String>(())
                    })
                    .await
                {
                    tracing::error!(%error, "native window teardown did not finish");
                    owner
                        .update(cx, |this, cx| {
                            this.switching_graphics = false;
                            this.capture_status = format!("Could not detach UI: {error}");
                            this.finish_quit_if_idle(cx);
                        })
                        .ok();
                    return;
                }
                let result = cx
                    .update(|cx| cx.request_windowing(gpui::WindowingRequest::Headless))
                    .await;
                owner
                    .update(cx, |this, cx| {
                        this.switching_graphics = false;
                        if result.is_ok() {
                            this.graphics = false;
                        } else {
                            tracing::warn!(?result, "could not detach graphics");
                        }
                        this.finish_quit_if_idle(cx);
                    })
                    .ok();
            })
            .detach();
        });
    }

    #[cfg(any(test, not(target_os = "windows")))]
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_controller(Some(window.window_handle()), cx)
    }

    fn new_controller(control: Option<AnyWindowHandle>, cx: &mut Context<Self>) -> Self {
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
            resident: control.is_none(),
            graphics: control.is_some(),
            switching_graphics: false,
            control,
            preview_active: false,
            edit_selection: false,
            focus_handle: cx.focus_handle(),
            overlays: Vec::new(),
            editor: None,
            editor_closing: false,
            backend,
            xcap_displays: Vec::new(),
            xcap_windows: Vec::new(),
            refreshing: false,
            diagnostics: false,
            settings_loading: !cfg!(test),
            settings_saving: false,
            output_directory: if cfg!(test) {
                Some(app_core::capture_output_dir())
            } else {
                None
            },
            settings_status: String::new(),
            history_database: if cfg!(test) {
                None
            } else {
                app_core::history_database_path().ok()
            },
            history_visible: false,
            history_loading: false,
            history_mutating: false,
            history_action_status: String::new(),
            history_refresh_pending: false,
            history_has_more: false,
            history_entries: Vec::new(),
            history_status: String::new(),
            list_status: String::new(),
            capture_status: "Ready".into(),
            last_capture: None,
            last_saved: None,
            last_render: None,
            capturing: false,
            capture_delay: None,
            clipboard: Arc::new(Mutex::new(app_core::ImageClipboard::default())),
            copying: false,
            saving: false,
            quitting: false,
            save_status: String::new(),
            clipboard_status: "No capture yet".into(),
            #[cfg(target_os = "windows")]
            hotkey: None,
            hotkey_status: "Global hotkey: not tested on this platform".into(),
            #[cfg(target_os = "windows")]
            tray: None,
            tray_status: "Tray: not tested on this platform".into(),
        };
        shell.refresh_xcap_lists(cx);
        #[cfg(not(test))]
        cx.spawn(async move |this, cx| {
            let settings = cx
                .background_spawn(async { app_core::CaptureSettings::load() })
                .await;
            this.update(cx, |this, cx| {
                this.settings_loading = false;
                match settings {
                    Ok(settings) => {
                        this.backend = XcapBackend::new(&settings.output_directory);
                        this.output_directory = Some(settings.output_directory);
                    }
                    Err(error) => {
                        this.settings_status = format!(
                            "Could not load the save folder: {error:#}. Please choose a folder."
                        );
                        this.show_control(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        #[cfg(target_os = "windows")]
        match app_core::SpikeTray::new() {
            Ok(tray) => {
                shell.tray = Some(tray);
                shell.tray_status =
                    "Tray: Select region / Capture / Show control window / Quit".into();
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

            cx.spawn(async move |this, cx| {
                while let Some(event) = events.recv().await {
                    if this
                        .update(cx, |this, cx| match event {
                            app_core::DesktopEvent::Reopen => this.show_control(cx),
                            app_core::DesktopEvent::Hotkey(event) => {
                                match this
                                    .hotkey
                                    .as_ref()
                                    .and_then(|hotkey| hotkey.command(event))
                                {
                                    Some(app_core::CaptureCommand::Primary) => {
                                        this.capture_primary(cx)
                                    }
                                    Some(app_core::CaptureCommand::Region) => {
                                        #[cfg(all(test, target_os = "windows"))]
                                        if let Some(probe) = cx.try_global::<RegionLatencyProbe>() {
                                            probe.mark("region_command_received");
                                        }
                                        this.open_overlays(OverlayMode::Fullscreen, cx)
                                    }
                                    Some(app_core::CaptureCommand::EditRegion) => {
                                        this.select_region_to_edit(cx)
                                    }
                                    None => {}
                                }
                            }
                            app_core::DesktopEvent::Menu(event) => {
                                match this.tray.as_ref().and_then(|tray| tray.command(event)) {
                                    Some(app_core::TrayCommand::Capture) => {
                                        this.capture_primary(cx)
                                    }
                                    Some(app_core::TrayCommand::Region) => {
                                        this.open_overlays(OverlayMode::Fullscreen, cx)
                                    }
                                    Some(app_core::TrayCommand::EditRegion) => {
                                        this.select_region_to_edit(cx)
                                    }
                                    Some(app_core::TrayCommand::Show) => this.show_control(cx),
                                    Some(app_core::TrayCommand::Quit) => {
                                        cx.defer(close_application)
                                    }
                                    _ => {}
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
        shell
    }

    fn choose_output_directory(&mut self, cx: &mut Context<Self>) {
        if self.capture_delay.is_some() {
            return;
        }
        if self.capturing
            || self.copying
            || self.saving
            || self.quitting
            || self.settings_loading
            || self.settings_saving
        {
            return;
        }
        self.settings_saving = true;
        let dialog = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose screenshot folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let chosen = match dialog.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Ok(None)) => None,
                result => {
                    this.update(cx, |this, cx| {
                        this.settings_saving = false;
                        this.settings_status = format!("Folder selection failed: {result:?}");
                        this.finish_quit_if_idle(cx);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let result = if let Some(path) = chosen {
                cx.background_spawn(async move {
                    let settings = app_core::CaptureSettings::for_directory(path)
                        .map_err(|error| format!("{error:#}"))?;
                    settings.save().map_err(|error| format!("{error:#}"))?;
                    Ok::<_, String>(settings.output_directory)
                })
                .await
                .map(Some)
            } else {
                Ok(None)
            };
            this.update(cx, |this, cx| {
                this.settings_saving = false;
                match result {
                    Ok(Some(path)) => {
                        this.backend = XcapBackend::new(&path);
                        this.output_directory = Some(path);
                        this.settings_status = "Save folder updated".into();
                    }
                    Ok(None) => {}
                    Err(error) => {
                        this.settings_status = format!("Save folder unchanged: {error:#}")
                    }
                }
                this.finish_quit_if_idle(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn refresh_history(&mut self, before: Option<i64>, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        if self.history_loading || self.history_mutating {
            self.history_refresh_pending = true;
            return;
        }
        let Some(database) = self.history_database.clone() else {
            self.history_status = "History storage is unavailable".into();
            cx.notify();
            return;
        };
        self.history_loading = true;
        self.history_status = "Loading history…".into();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { app_core::capture_history_page(&database, before) })
                .await;
            this.update(cx, |this, cx| {
                this.history_loading = false;
                if this.quitting {
                    return;
                }
                match result {
                    Ok(mut entries) => {
                        this.history_has_more = entries.len() > 20;
                        entries.truncate(20);
                        this.history_entries = entries;
                        this.history_status = if this.history_entries.is_empty() {
                            "No saved captures yet".into()
                        } else {
                            "Saved captures · newest first".into()
                        };
                    }
                    Err(error) => {
                        this.history_status = format!("History could not be loaded: {error:#}")
                    }
                }
                if std::mem::take(&mut this.history_refresh_pending) {
                    this.refresh_history(None, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn refresh_xcap_lists(&mut self, cx: &mut Context<Self>) {
        if self.refreshing || self.quitting {
            return;
        }
        self.refreshing = true;
        self.list_status = "Loading displays and windows…".into();
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
                        errors.push(format!("Windows: {error}"));
                        this.xcap_windows.clear();
                    }
                }
                this.list_status = if errors.is_empty() {
                    format!(
                        "{} display(s), {} window(s) loaded",
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
        self.edit_selection = false;
        self.cancel_capture_delay(cx);
        let retire = self.resident && self.preview_active && self.control.is_none();
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
        if retire {
            self.retire_ui(cx);
        }
        cx.notify();
    }

    fn open_overlays(&mut self, mode: OverlayMode, cx: &mut Context<Self>) {
        if self.editor.is_some() || self.editor_closing {
            return;
        }
        if self.capture_delay.is_some() {
            return;
        }
        if self.settings_loading || self.settings_saving || self.output_directory.is_none() {
            return;
        }
        if self.capturing || self.copying || self.saving || self.quitting || self.switching_graphics
        {
            return;
        }
        if !self.graphics {
            self.attach_ui(UiRequest::Overlay(mode), cx);
            return;
        }
        if matches!(mode, OverlayMode::Fullscreen) {
            if self.edit_selection {
                #[cfg(target_os = "windows")]
                let pointed = app_core::cursor_position().and_then(|point| {
                    self.xcap_displays
                        .iter()
                        .find(|display| {
                            let [x, y] = point.map(i64::from);
                            x >= i64::from(display.x)
                                && x < i64::from(display.x) + i64::from(display.width)
                                && y >= i64::from(display.y)
                                && y < i64::from(display.y) + i64::from(display.height)
                        })
                        .map(|display| display.id.clone())
                });
                #[cfg(not(target_os = "windows"))]
                let pointed: Option<String> = None;
                if let Some(id) = pointed.or_else(|| {
                    cx.primary_display()
                        .map(|display| u64::from(display.id()).to_string())
                }) {
                    self.start_capture_with_preview(vec![CaptureJob::Display(id)], true, cx);
                }
                return;
            }
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

    fn select_region_to_edit(&mut self, cx: &mut Context<Self>) {
        if self.capturing
            || self.copying
            || self.saving
            || self.quitting
            || self.switching_graphics
            || self.preview_active
            || self.editor.is_some()
            || self.editor_closing
            || self.capture_delay.is_some()
            || self.settings_loading
            || self.settings_saving
            || self.output_directory.is_none()
        {
            return;
        }
        self.edit_selection = true;
        self.open_overlays(OverlayMode::Fullscreen, cx);
    }

    fn start_delayed_region(&mut self, cx: &mut Context<Self>) {
        if self.editor.is_some() || self.editor_closing {
            return;
        }
        if self.capture_delay.is_some()
            || self.capturing
            || self.copying
            || self.saving
            || self.quitting
            || self.switching_graphics
            || self.preview_active
            || self.settings_loading
            || self.settings_saving
            || self.output_directory.is_none()
        {
            return;
        }
        self.capture_status = "Region capture in 3 seconds… Press Escape to cancel.".into();
        self.capture_delay = Some(cx.spawn(async move |this, cx| {
            for remaining in (0..3).rev() {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if this.quitting {
                            this.capture_delay.take();
                            return;
                        }
                        if remaining == 0 {
                            this.capture_delay.take();
                            this.open_overlays(OverlayMode::Fullscreen, cx);
                        } else {
                            this.capture_status = format!(
                                "Region capture in {remaining} {}… Press Escape to cancel.",
                                if remaining == 1 { "second" } else { "seconds" }
                            );
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn cancel_capture_delay(&mut self, cx: &mut Context<Self>) {
        if self.capture_delay.take().is_some() {
            self.capture_status = "Delayed capture canceled".into();
            cx.notify();
        }
    }

    fn restore_control(&self, cx: &mut Context<Self>) {
        let Some(control) = self.control else {
            return;
        };
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
                    log_overlay_geometry(window, display_bounds, cx);
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
                    log_overlay_geometry(window, display_bounds, cx);
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
            if self.resident && self.control.is_none() {
                self.retire_ui(cx);
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

    fn replace_last_render(&mut self, render: Arc<gpui::RenderImage>, cx: &mut Context<Self>) {
        if self.resident && self.control.is_none() {
            return;
        }
        if let Some(previous) = self.last_render.replace(render) {
            let Some(control) = self.control else {
                return;
            };
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

    fn finish_quit_if_idle(&mut self, cx: &mut Context<Self>) {
        if self.quitting
            && !self.capturing
            && !self.copying
            && !self.saving
            && !self.switching_graphics
            && !self.settings_saving
            && !self.history_mutating
            && !self.editor_closing
        {
            cx.defer(close_application);
        } else if !self.quitting
            && self.resident
            && self.control.is_none()
            && self.graphics
            && self.editor.is_none()
            && !self.editor_closing
            && !self.preview_active
            && !self.capturing
            && !self.copying
            && !self.saving
            && !self.switching_graphics
            && !self.settings_saving
            && !self.history_mutating
        {
            self.retire_ui(cx);
        }
    }

    fn copy_last_capture(&mut self, cx: &mut Context<Self>) {
        self.copy_and_save(Vec::new(), cx);
    }

    fn open_redaction_editor(&mut self, cx: &mut Context<Self>) {
        self.open_redaction_source(self.last_capture.clone(), self.last_saved.clone(), cx);
    }

    fn open_history_editor(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.open_redaction_source(None, Some(path), cx);
    }

    fn open_redaction_source(
        &mut self,
        source: Option<CapturedImage>,
        path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.editor.is_some()
            || self.editor_closing
            || self.capturing
            || self.copying
            || self.saving
            || self.quitting
            || self.capture_delay.is_some()
            || self.settings_saving
            || self.history_mutating
        {
            return;
        }
        if source.is_none() && path.is_none() {
            return;
        }
        let Some(directory) = self.output_directory.clone() else {
            return;
        };
        self.capturing = true;
        self.capture_status = "Opening image editor…".into();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut source = match source {
                        Some(source) => source,
                        None => {
                            let pixels = app_core::load_saved_png(&path.unwrap())
                                .map_err(|e| format!("{e:#}"))?;
                            CapturedImage {
                                width: pixels.width(),
                                height: pixels.height(),
                                pixels: Arc::new(pixels),
                                display_id: "saved-image".into(),
                                window_id: None,
                                png_path: directory.join("source.png"),
                                elapsed_ms: 0,
                                source_format: app_core::PixelFormat::Rgba8UnormSrgb,
                            }
                        }
                    };
                    source.png_path = directory.join("source.png");
                    let render = render_frame(&source);
                    Ok::<_, String>((source, render))
                })
                .await;
            this.update(cx, |this, cx| {
                this.capturing = false;
                if this.quitting {
                    this.finish_quit_if_idle(cx);
                    return;
                }
                match result {
                    Ok((source, render)) => this.show_prepared_editor(source, render, false, cx),
                    Err(error) => {
                        this.capture_status = format!("Could not load image: {error}");
                        this.history_action_status = this.capture_status.clone();
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn show_prepared_editor(
        &mut self,
        source: CapturedImage,
        render: Arc<gpui::RenderImage>,
        direct_capture: bool,
        cx: &mut Context<Self>,
    ) {
        let owner = cx.entity().downgrade();
        let close_owner = owner.clone();
        let bounds = Bounds::centered(None, size(px(1000.0), px(760.0)), cx);
        let options = if direct_capture {
            let display = cx
                .displays()
                .into_iter()
                .find(|display| u64::from(display.id()).to_string() == source.display_id);
            let Some(display) = display else {
                self.capture_status = "Capture display is no longer available".into();
                return;
            };
            overlay_options(OverlayMode::Fullscreen, display.id(), display.bounds())
        } else {
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(760.0), px(400.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Edit a copy — screenshots".into()),
                    ..Default::default()
                }),
                ..Default::default()
            }
        };
        match cx.open_window(options, |window, cx| {
            window.on_window_should_close(cx, move |_, cx| {
                let owner = close_owner.clone();
                cx.defer(move |cx| {
                    owner.update(cx, |this, cx| this.close_editor(cx)).ok();
                });
                false
            });
            cx.new(|cx| RedactionEditor::new(source, render, owner, direct_capture, window, cx))
        }) {
            Ok(handle) => {
                self.editor = Some(handle);
                self.capture_status = if direct_capture {
                    "Editor open. Original capture is not saved."
                } else {
                    "Image editor open. Original remains unchanged."
                }
                .into();
            }
            Err(error) => self.capture_status = format!("Could not open editor: {error}"),
        }
    }
    fn close_editor(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.editor.take() else {
            return;
        };
        self.editor_closing = true;
        self.capture_status = "Editor closed. Original remains unchanged.".into();
        self.history_action_status = self.capture_status.clone();
        let owner = cx.entity().downgrade();
        cx.defer(move |cx| {
            let hidden = handle
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
                        Ok::<_, String>(())
                    }
                })
                .map_err(|error| error.to_string())
                .and_then(|result| result);
            cx.spawn(async move |cx| {
                #[cfg(target_os = "windows")]
                let settled = cx
                    .background_spawn(async move {
                        let native = hidden?;
                        native
                            .wait_until_hidden()
                            .map_err(|error| error.to_string())?;
                        app_core::wait_for_desktop_compositor()
                            .map_err(|error| error.to_string())?;
                        app_core::wait_for_desktop_compositor()
                            .map_err(|error| error.to_string())?;
                        Ok::<_, String>(native)
                    })
                    .await;
                #[cfg(not(target_os = "windows"))]
                let settled = hidden;
                let native = match settled {
                    Ok(native) => native,
                    Err(error) => {
                        owner
                            .update(cx, |this, cx| {
                                this.editor_closing = false;
                                this.quitting = false;
                                this.editor = Some(handle);
                                this.capture_status = format!("Could not close editor: {error}");
                                this.restore_control(cx);
                                cx.notify();
                            })
                            .ok();
                        return;
                    }
                };
                handle
                    .update(cx, |editor, window, _| {
                        editor.release_images(window);
                        window.remove_window();
                    })
                    .ok();
                #[cfg(target_os = "windows")]
                let destroyed = cx
                    .background_spawn(async move { native.wait_until_destroyed() })
                    .await;
                #[cfg(not(target_os = "windows"))]
                let destroyed = {
                    let _ = native;
                    Ok::<_, String>(())
                };
                owner
                    .update(cx, |this, cx| {
                        this.editor_closing = false;
                        if let Err(error) = destroyed {
                            this.capture_status = format!("Editor cleanup failed: {error}");
                        }
                        this.restore_control(cx);
                        this.finish_quit_if_idle(cx);
                        cx.notify();
                    })
                    .ok();
            })
            .detach();
        });
        cx.notify();
    }
    fn export_redaction(
        &mut self,
        source: CapturedImage,
        regions: Vec<app_core::PixelRegion>,
        crop: Option<app_core::PixelRegion>,
        annotations: Vec<app_core::DrawAnnotation>,
        pixelations: Vec<app_core::PixelRegion>,
        pixelation: Option<Arc<app_core::PixelationGrid>>,
        blurs: Vec<app_core::PixelRegion>,
        mode: OutputMode,
        cx: &mut Context<Self>,
    ) {
        if self.capturing || self.copying || self.saving || self.quitting {
            return;
        }
        self.close_editor(cx);
        self.capturing = true;
        self.capture_status = "Applying image edits…".into();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    app_core::render_ordered_snapshot(
                        &source,
                        &regions,
                        crop,
                        &annotations,
                        &pixelations,
                        pixelation.as_deref(),
                        &blurs,
                    )
                    .map(|shot| {
                        let thumbnail = render_thumbnail(&shot);
                        (shot, thumbnail)
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                this.capturing = false;
                if this.quitting {
                    this.finish_quit_if_idle(cx);
                    return;
                }
                match result {
                    Ok((shot, thumbnail)) => {
                        this.replace_last_render(thumbnail, cx);
                        this.last_saved = None;
                        this.last_capture = Some(shot.clone());
                        this.capture_status = "Edited copy prepared. Original file is kept.".into();
                        if !mode.saves() {
                            this.save_status = "Not saved · Edited image is kept in memory".into();
                        }
                        if !mode.copies() {
                            this.clipboard_status = "Clipboard unchanged (Save only)".into();
                        }
                        this.output_captures(
                            if mode.saves() { vec![shot] } else { Vec::new() },
                            mode.copies(),
                            cx,
                        );
                    }
                    Err(error) => this.capture_status = format!("Image editing failed: {error:#}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn copy_history_capture(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.capturing
            || self.capture_delay.is_some()
            || self.copying
            || self.saving
            || self.quitting
            || self.history_mutating
        {
            return;
        }
        self.copying = true;
        self.history_action_status = "Copying image…".into();
        let clipboard = self.clipboard.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    clipboard
                        .lock()
                        .map_err(|error| error.to_string())?
                        .copy_saved_png(&path)
                        .map_err(|error| format!("{error:#}"))
                })
                .await;
            this.update(cx, |this, cx| {
                this.copying = false;
                let status = match result {
                    Ok((width, height)) => format!("Copied {width} × {height} to clipboard"),
                    Err(error) => format!("Could not copy image: {error}"),
                };
                this.clipboard_status = status.clone();
                this.history_action_status = status;
                this.finish_quit_if_idle(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn remove_history_capture(&mut self, id: i64, cx: &mut Context<Self>) {
        if self.quitting
            || self.history_loading
            || self.history_mutating
            || self.capturing
            || self.copying
            || self.saving
            || self.capture_delay.is_some()
        {
            return;
        }
        let Some(database) = self.history_database.clone() else {
            return;
        };
        self.history_mutating = true;
        self.history_action_status = "Removing history entry…".into();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { app_core::remove_history_entry(&database, id) })
                .await;
            this.update(cx, |this, cx| {
                this.history_mutating = false;
                match result {
                    Ok(()) => {
                        this.history_action_status = "Entry removed. Image file kept.".into();
                        this.refresh_history(None, cx);
                    }
                    Err(error) => {
                        this.history_action_status = format!("Could not remove entry: {error:#}")
                    }
                }
                this.finish_quit_if_idle(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn import_history_images(&mut self, cx: &mut Context<Self>) {
        if self.quitting
            || self.history_loading
            || self.history_mutating
            || self.capturing
            || self.copying
            || self.saving
            || self.capture_delay.is_some()
            || self.settings_saving
        {
            return;
        }
        let Some(database) = self.history_database.clone() else {
            return;
        };
        self.history_mutating = true;
        let dialog = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add PNG images to history (up to 100)".into()),
        });
        cx.spawn(async move |this, cx| {
            let selected = match dialog.await {
                Ok(Ok(paths)) => paths,
                error => {
                    this.update(cx, |this, cx| {
                        this.history_mutating = false;
                        this.history_action_status = format!("Image selection failed: {error:?}");
                        this.finish_quit_if_idle(cx);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let Some(paths) = selected else {
                this.update(cx, |this, cx| {
                    this.history_mutating = false;
                    this.finish_quit_if_idle(cx);
                    cx.notify();
                })
                .ok();
                return;
            };
            this.update(cx, |this, cx| {
                this.history_action_status = "Adding images…".into();
                cx.notify();
            })
            .ok();
            let result = cx
                .background_spawn(async move { app_core::import_capture_files(&database, &paths) })
                .await;
            this.update(cx, |this, cx| {
                this.history_mutating = false;
                match result {
                    Ok(report) => {
                        this.history_action_status = format!(
                            "Added {} · Already listed {} · Skipped {}",
                            report.added,
                            report.existing,
                            report.failures.len()
                        );
                        for (path, error) in report.failures.iter().take(3) {
                            this.history_action_status.push_str(&format!(
                                "\n{}: {error}",
                                path.file_name().unwrap_or_default().to_string_lossy()
                            ));
                        }
                        if report.failures.len() > 3 {
                            this.history_action_status
                                .push_str("\nOnly the first three errors are shown.");
                        }
                        this.refresh_history(None, cx);
                    }
                    Err(error) => this.history_action_status = format!("Import failed: {error:#}"),
                }
                this.finish_quit_if_idle(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn save_capture_as(&mut self, cx: &mut Context<Self>) {
        if self.capture_delay.is_some() {
            return;
        }
        if self.capturing || self.copying || self.saving || self.settings_saving || self.quitting {
            return;
        }
        let shot = self.last_capture.clone();
        let Some(source) = shot
            .as_ref()
            .map(|shot| shot.png_path.clone())
            .or_else(|| self.last_saved.clone())
        else {
            return;
        };
        let directory = self.output_directory.clone().unwrap_or_else(|| {
            source
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .to_path_buf()
        });
        let dialog = cx.prompt_for_new_path(&directory, Some("Screenshot.png"));
        let history_database = self.history_database.clone();
        self.saving = true;
        cx.spawn(async move |this, cx| {
            let chosen = match dialog.await {
                Ok(Ok(path)) => path,
                error => {
                    this.update(cx, |this, cx| {
                        this.saving = false;
                        this.save_status = format!("Save dialog failed: {error:?}");
                        this.finish_quit_if_idle(cx);
                        cx.notify();
                    }).ok();
                    return;
                }
            };
            let Some(mut path) = chosen else {
                this.update(cx, |this, cx| { this.saving = false; this.finish_quit_if_idle(cx); cx.notify(); }).ok();
                return;
            };
            if !path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("png")) { path.set_extension("png"); }
            let result = cx.background_spawn(async move {
                let result = if let Some(shot) = shot { shot.save_png_as(&path) } else { app_core::export_saved_png(&source, &path) };
                result.map(|_| {
                    let history_error = history_database.as_ref().and_then(|database| app_core::record_saved_capture(database, &path).err()).map(|error| format!("{error:#}"));
                    (path, history_error)
                }).map_err(|error| match error {
                    app_core::CaptureError::Io(error) if error.kind() == std::io::ErrorKind::AlreadyExists =>
                        "The file already exists. Choose a new name; existing files are never overwritten.".to_owned(),
                    error => error.to_string(),
                })
            }).await;
            this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok((path, history_error)) => {
                        this.save_status = format!("PNG saved: {}", path.display());
                        this.last_saved = Some(path);
                        this.last_capture = None;
                        if let Some(error) = history_error {
                            this.history_status = format!("PNG saved, but history was not updated: {error}");
                            this.save_status.push_str(" · History update failed");
                        } else if this.history_visible { this.refresh_history(None, cx); }
                    }
                    Err(error) => {
                        this.quitting = false;
                        this.save_status = format!("PNG not saved: {error}");
                        this.restore_control(cx);
                        if this.resident && this.control.is_none() { this.show_control(cx); }
                    }
                }
                this.finish_quit_if_idle(cx);
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }

    fn copy_and_save(&mut self, saves: Vec<CapturedImage>, cx: &mut Context<Self>) {
        self.output_captures(saves, true, cx);
    }

    fn output_captures(&mut self, saves: Vec<CapturedImage>, copy: bool, cx: &mut Context<Self>) {
        if self.capture_delay.is_some() || self.copying || self.saving || self.quitting {
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
        if copy {
            tracing::info!(path = %path.display(), "copying capture to clipboard");
        }
        let clipboard = self.clipboard.clone();
        self.copying = copy;
        self.saving = !saves.is_empty();
        if self.saving {
            self.save_status = if copy {
                "Save: pending clipboard completion"
            } else {
                "Saving PNG…"
            }
            .into();
        }
        if copy {
            self.clipboard_status = "Clipboard: copying…".into();
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            if copy {
                let result = cx
                    .background_spawn(async move {
                        let mut clipboard = clipboard.lock().map_err(|error| error.to_string())?;
                        match pixels {
                            Some(pixels) => clipboard.copy_image(&pixels),
                            None => clipboard.copy_saved_png(&path),
                        }
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
                            if this.resident && this.control.is_none() {
                                this.show_control(cx);
                            }
                            format!("Clipboard failed: {error}")
                        }
                    };
                    this.finish_quit_if_idle(cx);
                    cx.notify();
                })
                .ok();
            }
            if !saves.is_empty() {
                let history_database = this
                    .update(cx, |this, _| this.history_database.clone())
                    .ok()
                    .flatten();
                let results = cx
                    .background_spawn(async move {
                        saves
                            .into_iter()
                            .map(|shot| {
                                let saved = shot.save_png().map_err(|error| error.to_string());
                                let history_error = if saved.is_ok() {
                                    history_database
                                        .as_ref()
                                        .and_then(|database| {
                                            app_core::record_saved_capture(database, &shot.png_path)
                                                .err()
                                        })
                                        .map(|error| format!("{error:#}"))
                                } else {
                                    None
                                };
                                (shot.png_path.clone(), saved, history_error)
                            })
                            .collect::<Vec<_>>()
                    })
                    .await;
                this.update(cx, |this, cx| {
                    this.saving = false;
                    let errors: Vec<_> = results
                        .iter()
                        .filter_map(|(path, result, _)| {
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
                        if this.resident && this.control.is_none() {
                            this.show_control(cx);
                        }
                        tracing::error!(errors = %errors.join("; "), "capture output save failed");
                        format!("Save failed: {}", errors.join("; "))
                    };
                    let history_errors: Vec<_> = results
                        .iter()
                        .filter_map(|(_, _, error)| error.as_deref())
                        .collect();
                    if !history_errors.is_empty() {
                        this.save_status.push_str(" · History update failed");
                        this.history_status = format!(
                            "PNG saved, but history was not updated: {}",
                            history_errors.join("; ")
                        );
                    } else if this.history_visible {
                        this.refresh_history(None, cx);
                    }
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
        let edit_selection = self.edit_selection;
        // Keep graphics attached while the selected pixels move into the editor.
        if edit_selection {
            self.preview_active = false;
        }
        self.close_overlays(cx);
        self.capturing = true;
        self.capture_status = "Preparing selection…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { app_core::crop_snapshot(&source, region).map(|shot| { let render = if edit_selection { render_frame(&shot) } else { render_thumbnail(&shot) }; (shot, render) }) }).await;
            this.update(cx, |this, cx| {
                this.capturing = false;
                if this.quitting { this.finish_quit_if_idle(cx); return; }
                match result {
                    Ok((shot, render)) => {
                        if edit_selection {
                            this.show_prepared_editor(shot, render, true, cx);
                            if this.editor.is_none() { this.show_control(cx); }
                            this.finish_quit_if_idle(cx);
                            cx.notify();
                            return;
                        }
                        this.replace_last_render(render, cx);
                        tracing::info!(width = shot.width, height = shot.height, path = %shot.png_path.display(), "selection prepared");
                        this.capture_status = format!("Selection {}×{}  {}", shot.width, shot.height, shot.png_path.display());
                        this.last_capture = Some(shot.clone());
                        this.copy_and_save(vec![shot], cx);
                    }
                    Err(error) => {
                        tracing::error!(%error, "selection failed");
                        this.capture_status = format!("Selection failed: {error:#}");
                        if this.control.is_none() { this.show_control(cx); }
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
        if self.editor.is_some() || self.editor_closing {
            return;
        }
        if self.capture_delay.is_some() {
            return;
        }
        if self.settings_loading || self.settings_saving || self.output_directory.is_none() {
            return;
        }
        if self.capturing || self.copying || self.saving || self.quitting || self.switching_graphics
        {
            return;
        }
        if jobs.is_empty() {
            return;
        }
        // A new capture replaces the preview; keep the control window hidden.
        let edit_selection = std::mem::take(&mut self.edit_selection) && preview;
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
        #[cfg(all(test, target_os = "windows"))]
        let probe = cx.try_global::<RegionLatencyProbe>().cloned();
        #[cfg(target_os = "windows")]
        let mut hidden_windows: Vec<_> = self
            .overlays
            .iter()
            .filter_map(|handle| handle.read(cx).ok().and_then(|overlay| overlay.native))
            .collect();
        cx.spawn(async move |this, cx| {
            // Defer until the button/action has released its borrow of the control window.
            let hidden = if let Some(control) = control {
                control
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
                    .and_then(|result| result)
                    .map(Some)
            } else {
                Ok(None)
            };
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
            hidden_windows.extend(_hidden_control);
            // shortcut: other platforms still need a native visibility/compositor spike.
            #[cfg(not(target_os = "windows"))]
            cx.background_executor()
                .timer(std::time::Duration::from_millis(300))
                .await;
            let result = cx
                .background_spawn(async move {
                    #[cfg(all(test, target_os = "windows"))]
                    if let Some(probe) = &probe {
                        probe.mark("worker_started");
                    }
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
                    #[cfg(all(test, target_os = "windows"))]
                    if let Some(probe) = &probe {
                        probe.mark("hidden_and_compositor_ready");
                    }
                    let captured = backend.capture_batch(jobs)?;
                    #[cfg(all(test, target_os = "windows"))]
                    if let Some(probe) = &probe {
                        probe.mark("capture_complete");
                    }
                    for shot in captured {
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
                    #[cfg(all(test, target_os = "windows"))]
                    if let Some(probe) = &probe {
                        probe.mark("render_conversion_complete");
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
                            if edit_selection {
                                if let Some(render) = renders.get(&shot.png_path).cloned() {
                                    this.show_prepared_editor(shot, render, true, cx);
                                }
                                if this.editor.is_none() {
                                    this.show_control(cx);
                                }
                                this.finish_quit_if_idle(cx);
                                cx.notify();
                                return;
                            }
                            this.show_overlays(OverlayMode::Fullscreen, &shots, &renders, cx);
                            this.edit_selection = edit_selection && this.preview_active;
                            for handle in &this.overlays {
                                handle
                                    .update(cx, |overlay, _, cx| {
                                        overlay.edit_selection = edit_selection;
                                        cx.notify();
                                    })
                                    .ok();
                            }
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
                        if this.resident && this.control.is_none() {
                            this.show_control(cx);
                        }
                    }
                }

                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Focusable for SpikeShell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SpikeShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.capturing
            || self.editor.is_some()
            || self.editor_closing
            || self.capture_delay.is_some()
            || self.copying
            || self.saving
            || self.quitting
            || self.settings_loading
            || self.settings_saving
            || self.switching_graphics
            || self.history_mutating;
        let unavailable = busy || self.output_directory.is_none();
        let displays = self.xcap_displays.clone();
        let windows: Vec<_> = self
            .xcap_windows
            .iter()
            .filter(|window| {
                !window.is_minimized && !window.title.is_empty() && window.pid != std::process::id()
            })
            .map(|window| {
                (
                    window.id.clone(),
                    window.title.chars().take(70).collect::<String>(),
                )
            })
            .collect();
        let folder = self
            .output_directory
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "Loading save folder…".into());
        let last_saved = self.last_saved.clone();
        div()
            .id("capture-home")
            .track_focus(&self.focus_handle)
            .key_context("SpikeShell")
            .flex()
            .flex_col()
            .size_full()
            .p_6()
            .gap_4()
            .overflow_y_scroll()
            .bg(rgb(0x111318))
            .text_color(rgb(0xf1f5f9))
            .on_action(cx.listener(|this, _: &OpenProbeOverlays, _, cx| {
                this.open_overlays(OverlayMode::Probe, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenFullscreenOverlays, _, cx| {
                this.open_overlays(OverlayMode::Fullscreen, cx)
            }))
            .on_action(cx.listener(|this, _: &CloseOverlay, _, cx| this.close_overlays(cx)))
            .on_action(cx.listener(|this, _: &CapturePrimary, _, cx| this.capture_primary(cx)))
            .on_action(cx.listener(|this, _: &CaptureAll, _, cx| this.capture_all(cx)))
            .on_action(
                cx.listener(|this, _: &CaptureCenterRegion, _, cx| this.capture_center_region(cx)),
            )
            .on_action(|_: &Quit, _, cx| cx.defer(close_application))
            .child(div().text_2xl().child("Screenshots"))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x94a3b8))
                    .child("Capture. Copy. Done."),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(capture_button(
                        "select-region",
                        "Select region",
                        unavailable,
                        cx.listener(|this, _, _, cx| {
                            this.open_overlays(OverlayMode::Fullscreen, cx)
                        }),
                    ))
                    .child(capture_button(
                        "select-region-to-edit",
                        "Capture and annotate",
                        unavailable,
                        cx.listener(|this, _, _, cx| this.select_region_to_edit(cx)),
                    ))
                    .child(capture_button(
                        "delayed-region",
                        "Region in 3 seconds",
                        unavailable,
                        cx.listener(|this, _, _, cx| this.start_delayed_region(cx)),
                    ))
                    .child(capture_button(
                        "capture-primary",
                        "Primary display",
                        unavailable,
                        cx.listener(|this, _, _, cx| this.capture_primary(cx)),
                    ))
                    .child(capture_button(
                        "capture-all",
                        "All displays",
                        unavailable,
                        cx.listener(|this, _, _, cx| this.capture_all(cx)),
                    )),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x94a3b8))
                    .child(self.hotkey_status.clone()),
            )
            .child(div().text_sm().child(self.capture_status.clone()))
            .when(self.capture_delay.is_some(), |view| {
                view.child(capture_button(
                    "cancel-delayed-capture",
                    "Cancel countdown",
                    false,
                    cx.listener(|this, _, _, cx| this.cancel_capture_delay(cx)),
                ))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p_4()
                    .gap_2()
                    .rounded_lg()
                    .bg(rgb(0x1d222c))
                    .child("Last capture")
                    .children(
                        self.last_render
                            .clone()
                            .map(|render| img(render).h(px(160.0)).max_w_full().rounded_md()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(capture_button(
                                "copy-last-capture",
                                "Copy again",
                                busy || (self.last_capture.is_none() && self.last_saved.is_none()),
                                cx.listener(|this, _, _, cx| this.copy_last_capture(cx)),
                            ))
                            .child(capture_button(
                                "redact-last-capture",
                                "Edit a copy",
                                busy || (self.last_capture.is_none() && self.last_saved.is_none()),
                                cx.listener(|this, _, _, cx| this.open_redaction_editor(cx)),
                            ))
                            .child(capture_button(
                                "save-capture-as",
                                "Save as…",
                                busy || (self.last_capture.is_none() && self.last_saved.is_none()),
                                cx.listener(|this, _, _, cx| this.save_capture_as(cx)),
                            ))
                            .child(capture_button(
                                "reveal-last-capture",
                                "Show file",
                                last_saved.is_none(),
                                move |_, _, cx| {
                                    if let Some(path) = &last_saved {
                                        cx.reveal_path(path);
                                    }
                                },
                            )),
                    )
                    .child(div().text_sm().child(self.clipboard_status.clone()))
                    .child(div().text_sm().child(self.save_status.clone())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child("Capture a display")
                    .children(displays.into_iter().enumerate().map(|(index, display)| {
                        let id = display.id;
                        div()
                            .id(("display-capture", index))
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(0x242b36))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !unavailable {
                                    this.start_capture(vec![CaptureJob::Display(id.clone())], cx);
                                }
                            }))
                            .child(format!(
                                "{} · {} × {}{}",
                                display.name,
                                display.width,
                                display.height,
                                if display.is_primary {
                                    " · Primary display"
                                } else {
                                    ""
                                }
                            ))
                    })),
            )
            .when(self.backend.capabilities().capture_window, |view| {
                view.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .items_center()
                                .child("Capture a window")
                                .child(capture_button(
                                    "refresh-windows",
                                    "Refresh",
                                    busy || self.refreshing,
                                    cx.listener(|this, _, _, cx| this.refresh_xcap_lists(cx)),
                                )),
                        )
                        .child(
                            div()
                                .id("window-picker")
                                .max_h(px(180.0))
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .children(windows.into_iter().enumerate().map(
                                    |(index, (id, title))| {
                                        div()
                                            .id(("capture-window", index))
                                            .flex_none()
                                            .px_3()
                                            .py_2()
                                            .rounded_md()
                                            .bg(rgb(0x242b36))
                                            .cursor_pointer()
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if !unavailable {
                                                    this.start_capture(
                                                        vec![CaptureJob::Window(id.clone())],
                                                        cx,
                                                    );
                                                }
                                            }))
                                            .child(title)
                                    },
                                )),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x94a3b8))
                                .child(self.list_status.clone()),
                        ),
                )
            })
            .child(capture_button(
                "toggle-history",
                if self.history_visible {
                    "Hide history"
                } else {
                    "Capture history"
                },
                false,
                cx.listener(|this, _, _, cx| {
                    this.history_visible = !this.history_visible;
                    if this.history_visible {
                        this.refresh_history(None, cx);
                    }
                    cx.notify();
                }),
            ))
            .when(self.history_visible, |view| {
                view.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(div().text_lg().child("Capture history"))
                        .child(capture_button(
                            "import-history-images",
                            "Add PNGs…",
                            busy || self.history_loading,
                            cx.listener(|this, _, _, cx| this.import_history_images(cx)),
                        ))
                        .child(div().text_sm().child(self.history_status.clone()))
                        .child(
                            div()
                                .text_sm()
                                .text_color(rgb(0x94a3b8))
                                .child("Removing an entry keeps the image file."),
                        )
                        .child(div().text_sm().child(self.history_action_status.clone()))
                        .child(
                            div()
                                .id("capture-history-list")
                                .max_h(px(320.0))
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .children(
                                    self.history_entries.clone().into_iter().enumerate().map(
                                        |(index, entry)| {
                                            let open_path = entry.path.clone();
                                            let reveal_path = entry.path.clone();
                                            let copy_path = entry.path.clone();
                                            let edit_path = entry.path.clone();
                                            let entry_id = entry.id;
                                            div()
                                                .id(("history-entry", index))
                                                .flex_none()
                                                .flex()
                                                .flex_col()
                                                .p_3()
                                                .gap_1()
                                                .rounded_md()
                                                .bg(rgb(0x242b36))
                                                .child(
                                                    entry
                                                        .path
                                                        .file_name()
                                                        .unwrap_or_default()
                                                        .to_string_lossy()
                                                        .into_owned(),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .text_color(rgb(0x94a3b8))
                                                        .child(format!(
                                                            "{} × {} · {}",
                                                            entry.width,
                                                            entry.height,
                                                            entry.created_at
                                                        )),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .gap_2()
                                                        .child(capture_button(
                                                            "open-history-file",
                                                            "Open",
                                                            false,
                                                            move |_, _, cx| {
                                                                cx.open_with_system(&open_path)
                                                            },
                                                        ))
                                                        .child(capture_button(
                                                            "edit-history-file",
                                                            "Edit a copy",
                                                            busy,
                                                            cx.listener(move |this, _, _, cx| {
                                                                this.open_history_editor(
                                                                    edit_path.clone(),
                                                                    cx,
                                                                )
                                                            }),
                                                        ))
                                                        .child(capture_button(
                                                            "copy-history-file",
                                                            "Copy",
                                                            busy,
                                                            cx.listener(move |this, _, _, cx| {
                                                                this.copy_history_capture(
                                                                    copy_path.clone(),
                                                                    cx,
                                                                )
                                                            }),
                                                        ))
                                                        .child(capture_button(
                                                            "remove-history-entry",
                                                            "Remove entry",
                                                            busy || self.history_loading,
                                                            cx.listener(move |this, _, _, cx| {
                                                                this.remove_history_capture(
                                                                    entry_id, cx,
                                                                )
                                                            }),
                                                        ))
                                                        .child(capture_button(
                                                            "reveal-history-file",
                                                            "Show file",
                                                            false,
                                                            move |_, _, cx| {
                                                                cx.reveal_path(&reveal_path)
                                                            },
                                                        )),
                                                )
                                        },
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(capture_button(
                                    "history-latest",
                                    "Latest",
                                    self.history_loading || self.history_mutating,
                                    cx.listener(|this, _, _, cx| this.refresh_history(None, cx)),
                                ))
                                .child(capture_button(
                                    "history-older",
                                    "Older",
                                    self.history_loading
                                        || self.history_mutating
                                        || !self.history_has_more,
                                    cx.listener(|this, _, _, cx| {
                                        if let Some(entry) = this.history_entries.last() {
                                            this.refresh_history(Some(entry.id), cx);
                                        }
                                    }),
                                )),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p_4()
                    .gap_2()
                    .rounded_lg()
                    .bg(rgb(0x1d222c))
                    .child("Save folder")
                    .child(div().text_sm().child(folder))
                    .child(capture_button(
                        "choose-output-directory",
                        "Change folder…",
                        busy,
                        cx.listener(|this, _, _, cx| this.choose_output_directory(cx)),
                    ))
                    .child(div().text_sm().child(self.settings_status.clone())),
            )
            .child(capture_button(
                "toggle-diagnostics",
                if self.diagnostics {
                    "Hide diagnostics"
                } else {
                    "Diagnostics"
                },
                false,
                cx.listener(|this, _, _, cx| {
                    this.diagnostics = !this.diagnostics;
                    cx.notify();
                }),
            ))
            .when(self.diagnostics, |view| {
                view.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .text_sm()
                        .text_color(rgb(0x94a3b8))
                        .child(self.tray_status.clone())
                        .child(format!("{} overlay window(s)", self.overlays.len()))
                        .child(capture_button(
                            "open-probe",
                            "Open test overlay",
                            unavailable,
                            cx.listener(|this, _, _, cx| {
                                this.open_overlays(OverlayMode::Probe, cx)
                            }),
                        ))
                        .child(capture_button(
                            "close-overlays",
                            "Close overlays",
                            false,
                            cx.listener(|this, _, _, cx| this.close_overlays(cx)),
                        )),
                )
            })
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
    edit_selection: bool,
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
            edit_selection: false,
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
                            if self.edit_selection { "Drag to select. Release to edit without saving the original. Escape cancels." } else { "Drag to select. Release to save and copy. Escape cancels. Selection stays on one monitor." },
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

fn log_overlay_geometry(window: &Window, expected: Bounds<Pixels>, _cx: &App) {
    #[cfg(all(test, target_os = "windows"))]
    let probe = _cx.try_global::<RegionLatencyProbe>().cloned();
    #[cfg(all(test, target_os = "windows"))]
    if let Some(probe) = &probe {
        probe.mark("overlay_setup");
    }
    window.on_next_frame(move |window, _| {
        #[cfg(all(test, target_os = "windows"))]
        if let Some(probe) = &probe {
            probe.mark("overlay_first_callback");
        }
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
            #[cfg(all(test, target_os = "windows"))]
            if let Some(probe) = &probe {
                probe.mark("overlay_second_callback");
            }
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
    if cx.has_global::<ResidentController>() {
        let entity = cx.global::<ResidentController>().0.clone();
        let busy = entity.update(cx, |this, cx| {
            this.cancel_capture_delay(cx);
            this.close_editor(cx);
            this.quitting = true;
            let busy = this.capturing
                || this.copying
                || this.saving
                || this.switching_graphics
                || this.settings_saving
                || this.history_mutating
                || this.editor_closing;
            if busy {
                this.capture_status = "Quitting: waiting for active work…".into();
                cx.notify();
            }
            busy
        });
        if !busy {
            cx.quit();
        }
        return;
    }
    for handle in cx.windows() {
        if let Some(control) = handle.downcast::<SpikeShell>() {
            let waiting = control
                .update(cx, |this, _, cx| {
                    this.cancel_capture_delay(cx);
                    this.quitting = true;
                    let busy = this.capturing
                        || this.copying
                        || this.saving
                        || this.settings_saving
                        || this.history_mutating
                        || this.editor_closing;
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
    #[cfg(target_os = "windows")]
    {
        run_resident();
        return;
    }
    #[cfg(not(target_os = "windows"))]
    {
        tracing::info!(
            version = app_core::version(),
            "starting overlay + capture spike"
        );

        let platform = gpui_platform::current_platform(false);
        let fonts = caption::CaptionFonts(platform.text_system());
        gpui::Application::with_platform(platform).run(move |cx: &mut App| {
            cx.set_global(fonts);
            redaction_editor::bind_keys(cx);
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
                        title: Some("screenshots".into()),
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
}

#[cfg(target_os = "windows")]
fn run_resident() {
    let instance = match app_core::SingleInstance::acquire() {
        Ok(Some(instance)) => Arc::new(Mutex::new(instance)),
        Ok(None) => return,
        Err(error) => {
            tracing::error!(%error, "Could not establish single-instance ownership; app will not start");
            std::process::exit(1);
        }
    };
    let listener = instance.clone();
    let show = std::env::args().any(|arg| arg == "--show");
    let deadline = std::env::args()
        .find_map(|arg| {
            arg.strip_prefix("--exit-after=")
                .and_then(|value| value.parse::<u64>().ok())
        })
        .map(|seconds| seconds.clamp(10, 660));
    let platform = gpui_platform::current_platform(false);
    let fonts = caption::CaptionFonts(platform.text_system());
    gpui::Application::with_platform(platform)
        .with_windowing(gpui::WindowingRequest::Headless)
        .run(move |cx| {
            cx.set_global(fonts);
            cx.set_quit_mode(gpui::QuitMode::Explicit);
            redaction_editor::bind_keys(cx);
            let controller = cx.new(|cx| SpikeShell::new_controller(None, cx));
            cx.set_global(ResidentController(controller.clone()));
            if let Err(error) = listener.lock().unwrap().listen(app_core::request_reopen) {
                tracing::error!(%error, "Could not start launch listener");
                cx.quit();
                return;
            }
            cx.on_action(|_: &Quit, cx| cx.defer(close_application));
            cx.bind_keys([
                KeyBinding::new("escape", CloseOverlay, None),
                KeyBinding::new("ctrl-shift-o", OpenProbeOverlays, Some("SpikeShell")),
                KeyBinding::new("ctrl-shift-f", OpenFullscreenOverlays, Some("SpikeShell")),
                KeyBinding::new("ctrl-shift-c", CapturePrimary, Some("SpikeShell")),
                KeyBinding::new("ctrl-q", Quit, None),
            ]);
            controller.update(cx, |this, cx| {
                if show || this.tray.is_none() {
                    this.show_control(cx);
                }
            });
            if let Some(seconds) = deadline {
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(seconds))
                        .await;
                    cx.update(close_application);
                })
                .detach();
            }
        });
    // Keep ownership until GPUI has released its tray and hotkey registrations.
    drop(instance);
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
    fn track_gui_errors() -> Arc<std::sync::atomic::AtomicUsize> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tracing_subscriber::prelude::*;
        struct CountErrors(Arc<AtomicUsize>);
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CountErrors {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _: tracing_subscriber::layer::Context<'_, S>,
            ) {
                if *event.metadata().level() == tracing::Level::ERROR {
                    self.0.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        let errors = Arc::new(AtomicUsize::new(0));
        tracing_subscriber::registry()
            .with(CountErrors(errors.clone()))
            .with(tracing_subscriber::fmt::layer().with_test_writer())
            .with(tracing_subscriber::filter::LevelFilter::ERROR)
            .try_init()
            .unwrap();
        errors
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "writes a small fixture and replaces clipboard to verify independent output modes"]
    fn save_only_and_copy_only_have_separate_side_effects() {
        let errors = track_gui_errors();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/captures/output-modes-{}",
            probe_timestamp()
        ));
        let directory = root.clone();
        let result = Arc::new(Mutex::new(None));
        let report = result.clone();
        let before =
            unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        gpui_platform::application().with_windowing(gpui::WindowingRequest::Headless).run(move |cx| {
            cx.set_quit_mode(gpui::QuitMode::Explicit);
            let controller = cx.new(|cx| SpikeShell::new_controller(None, cx));
            cx.set_global(ResidentController(controller.clone()));
            controller.update(cx, |this, _| this.history_database = Some(directory.join("history.sqlite3")));
            let owner = controller.downgrade();
            cx.spawn(async move |cx| {
                let checked = async {
                    for copy_only in [false, true] {
                        owner.update(cx, |this, cx| {
                            let shot = CapturedImage { display_id: "fixture".into(), window_id: None, width: 2, height: 2,
                                pixels: Arc::new(image::RgbaImage::from_pixel(2, 2, image::Rgba([91, 42, 17, 255]))),
                                png_path: directory.join(if copy_only { "copy-only.png" } else { "save-only.png" }),
                                elapsed_ms: 0, source_format: app_core::PixelFormat::Rgba8UnormSrgb };
                            this.last_saved = None; this.last_capture = Some(shot.clone());
                            this.output_captures(if copy_only { Vec::new() } else { vec![shot] }, copy_only, cx);
                        }).map_err(|e| e.to_string())?;
                        let deadline = std::time::Instant::now();
                        loop {
                            cx.background_executor().timer(std::time::Duration::from_millis(20)).await;
                            if owner.update(cx, |this, _| !this.copying && !this.saving).map_err(|e| e.to_string())? { break; }
                            if deadline.elapsed().as_secs() > 5 { return Err("output timed out".to_string()); }
                        }
                        let state = owner.update(cx, |this, _| (this.last_saved.is_some(), this.last_capture.is_some())).map_err(|e| e.to_string())?;
                        if state != (!copy_only, copy_only) { return Err("incorrect output retention state".into()); }
                        if !copy_only && unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() } != before { return Err("Save only changed clipboard".into()); }
                    }
                    Ok(())
                }.await;
                *report.lock().unwrap() = Some(checked); cx.update(close_application);
            }).detach();
        });
        result
            .lock()
            .unwrap()
            .take()
            .expect("output mode test did not finish")
            .unwrap();
        assert!(root.join("save-only.png").exists());
        assert!(!root.join("copy-only.png").exists());
        assert_eq!(
            app_core::capture_history_page(&root.join("history.sqlite3"), None)
                .unwrap()
                .len(),
            1
        );
        assert_ne!(
            unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() },
            before
        );
        assert_eq!(errors.load(std::sync::atomic::Ordering::Relaxed), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "opens a real fullscreen annotation overlay; clipboard is not modified"]
    fn direct_edit_selection_cancel_has_no_output_side_effects() {
        let errors = track_gui_errors();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/captures/direct-edit-{}",
            probe_timestamp()
        ));
        let directory = root.clone();
        let clipboard =
            unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        let result = Arc::new(Mutex::new(None));
        let report = result.clone();
        let (application, fonts) = {
            let platform = gpui_platform::current_platform(false);
            let fonts = caption::CaptionFonts(platform.text_system());
            (gpui::Application::with_platform(platform), fonts)
        };
        application
            .with_windowing(gpui::WindowingRequest::Headless)
            .run(move |cx| {
                cx.set_global(fonts);
                cx.set_quit_mode(gpui::QuitMode::Explicit);
                let controller = cx.new(|cx| SpikeShell::new_controller(None, cx));
                cx.set_global(ResidentController(controller.clone()));
                controller.update(cx, |this, _| {
                    this.history_database = Some(directory.join("history.sqlite3"));
                    this.output_directory = Some(directory);
                    this.backend = XcapBackend::new(this.output_directory.as_ref().unwrap());
                });
                let owner = controller.downgrade();
                cx.spawn(async move |cx| {
                    let checked = async {
                        let deadline = std::time::Instant::now();
                        loop {
                            if owner
                                .update(cx, |this, _| !this.refreshing)
                                .map_err(|e| e.to_string())?
                            {
                                break;
                            }
                            if deadline.elapsed().as_secs() > 8 {
                                return Err("control startup timed out".to_string());
                            }
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(20))
                                .await;
                        }
                        owner
                            .update(cx, |this, cx| this.select_region_to_edit(cx))
                            .map_err(|e| e.to_string())?;
                        let deadline = std::time::Instant::now();
                        loop {
                            if owner
                                .update(cx, |this, _| this.editor.is_some() && !this.capturing)
                                .map_err(|e| e.to_string())?
                            {
                                break;
                            }
                            if deadline.elapsed().as_secs() > 8 {
                                return Err("direct editor startup timed out".into());
                            }
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(20))
                                .await;
                        }
                        let editor = owner
                            .update(cx, |this, _| this.editor.unwrap())
                            .map_err(|e| e.to_string())?;
                        let deadline = std::time::Instant::now();
                        loop {
                            if editor
                                .update(cx, |editor, window, cx| {
                                    editor.source_effects_probe(window, cx)
                                })
                                .map_err(|e| e.to_string())??
                            {
                                break;
                            }
                            if deadline.elapsed().as_secs() > 8 {
                                return Err("source effect preview timed out".into());
                            }
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(20))
                                .await;
                        }
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(40))
                            .await;
                        owner
                            .update(cx, |this, cx| this.close_editor(cx))
                            .map_err(|e| e.to_string())?;
                        let deadline = std::time::Instant::now();
                        loop {
                            let ready = owner
                                .update(cx, |this, cx| {
                                    !this.editor_closing
                                        && this.editor.is_none()
                                        && !this.switching_graphics
                                        && !this.graphics
                                        && cx.windows().is_empty()
                                })
                                .map_err(|e| e.to_string())?;
                            if ready {
                                break;
                            }
                            if deadline.elapsed().as_secs() > 5 {
                                return Err("editor cancel timed out".into());
                            }
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(20))
                                .await;
                        }
                        let unchanged = owner
                            .update(cx, |this, _| {
                                this.last_capture.is_none()
                                    && this.last_saved.is_none()
                                    && !this.saving
                                    && !this.copying
                                    && !this.edit_selection
                            })
                            .map_err(|e| e.to_string())?;
                        if !unchanged {
                            return Err("direct edit changed output state".into());
                        }
                        Ok(())
                    }
                    .await;
                    *report.lock().unwrap() = Some(checked);
                    cx.update(close_application);
                })
                .detach();
            });
        result
            .lock()
            .unwrap()
            .take()
            .expect("direct edit test did not finish")
            .unwrap();
        assert!(
            !root.exists(),
            "unmodified source or selection was written to disk"
        );
        assert_eq!(
            unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() },
            clipboard
        );
        assert_eq!(errors.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "opens an editor and replaces clipboard with a tiny redacted fixture"]
    fn redaction_editor_opens_and_exports_copy() {
        let errors = track_gui_errors();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/captures/redaction-test-{}",
            probe_timestamp()
        ));
        let output = root.clone();
        std::fs::create_dir_all(&root).unwrap();
        let selected_path = root.join("selected.png");
        image::RgbaImage::from_pixel(4, 3, image::Rgba([211, 109, 37, 255]))
            .save(&selected_path)
            .unwrap();
        let selected_original = std::fs::read(&selected_path).unwrap();
        let result = Arc::new(Mutex::new(None));
        let report = result.clone();
        gpui_platform::application()
            .with_windowing(gpui::WindowingRequest::Headless)
            .run(move |cx| {
                cx.set_quit_mode(gpui::QuitMode::Explicit);
                let controller = cx.new(|cx| SpikeShell::new_controller(None, cx));
                cx.set_global(ResidentController(controller.clone()));
                controller.update(cx, |this, cx| {
                    this.output_directory = Some(output.clone());
                    this.last_capture = Some(CapturedImage {
                        display_id: "fixture".into(),
                        window_id: None,
                        width: 4,
                        height: 3,
                        pixels: Arc::new(image::RgbaImage::from_pixel(
                            4,
                            3,
                            image::Rgba([17, 42, 91, 255]),
                        )),
                        png_path: output.join("source.png"),
                        elapsed_ms: 0,
                        source_format: app_core::PixelFormat::Rgba8UnormSrgb,
                    });
                    this.show_control(cx);
                });
                let owner = controller.downgrade();
                cx.spawn(async move |cx| {
                    let checked = async {
                        for phase in 0..3 {
                            let started = std::time::Instant::now();
                            loop {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(20))
                                    .await;
                                let ready = owner
                                    .update(cx, |this, _| match phase {
                                        0 => this.control.is_some() && !this.switching_graphics,
                                        1 => this.editor.is_some() && !this.capturing,
                                        _ => {
                                            !this.capturing
                                                && !this.copying
                                                && !this.saving
                                                && this.last_saved.is_some()
                                        }
                                    })
                                    .map_err(|e| e.to_string())?;
                                if ready {
                                    break;
                                }
                                if started.elapsed().as_secs() > 8 {
                                    return Err(format!("editor phase {phase} timed out"));
                                }
                            }
                            owner
                                .update(cx, |this, cx| match phase {
                                    0 => this.open_history_editor(
                                        this.output_directory
                                            .as_ref()
                                            .unwrap()
                                            .join("selected.png"),
                                        cx,
                                    ),
                                    1 => this.export_redaction(
                                        this.editor.unwrap().read(cx).unwrap().source.clone(),
                                        vec![app_core::PixelRegion {
                                            x: 0,
                                            y: 0,
                                            width: 2,
                                            height: 2,
                                        }],
                                        None,
                                        Vec::new(),
                                        Vec::new(),
                                        None,
                                        Vec::new(),
                                        OutputMode::SaveAndCopy,
                                        cx,
                                    ),
                                    _ => {}
                                })
                                .map_err(|e| e.to_string())?;
                        }
                        owner
                            .update(cx, |this, _| this.last_saved.clone().unwrap())
                            .map_err(|e| e.to_string())
                    }
                    .await;
                    *report.lock().unwrap() = Some(checked);
                    cx.update(close_application);
                })
                .detach();
            });
        let path = result
            .lock()
            .unwrap()
            .take()
            .expect("editor test did not finish")
            .unwrap();
        let image = image::open(path).unwrap().into_rgba8();
        assert_eq!(*image.get_pixel(0, 0), image::Rgba([0, 0, 0, 255]));
        assert_eq!(*image.get_pixel(3, 2), image::Rgba([211, 109, 37, 255]));
        assert_eq!(std::fs::read(selected_path).unwrap(), selected_original);
        assert_eq!(errors.load(std::sync::atomic::Ordering::Relaxed), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "opens one real region overlay after a countdown; no file or clipboard output"]
    fn delayed_region_can_cancel_and_complete() {
        let errors = track_gui_errors();
        let result = Arc::new(Mutex::new(None));
        let report = result.clone();
        gpui_platform::application()
            .with_windowing(gpui::WindowingRequest::Headless)
            .run(move |cx| {
                cx.set_quit_mode(gpui::QuitMode::Explicit);
                let controller = cx.new(|cx| SpikeShell::new_controller(None, cx));
                cx.set_global(ResidentController(controller.clone()));
                let owner = controller.downgrade();
                cx.spawn(async move |cx| {
                    let checked = async {
                        owner
                            .update(cx, |this, cx| {
                                this.start_delayed_region(cx);
                                assert!(this.capture_delay.is_some());
                                this.cancel_capture_delay(cx);
                            })
                            .map_err(|e| e.to_string())?;
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(3100))
                            .await;
                        let canceled = owner
                            .update(cx, |this, cx| {
                                this.capture_delay.is_none()
                                    && !this.capturing
                                    && !this.graphics
                                    && cx.windows().is_empty()
                            })
                            .map_err(|e| e.to_string())?;
                        if !canceled {
                            return Err("canceled countdown started capture".to_string());
                        }
                        owner
                            .update(cx, |this, cx| this.start_delayed_region(cx))
                            .map_err(|e| e.to_string())?;
                        let deadline = std::time::Instant::now();
                        loop {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(20))
                                .await;
                            if owner
                                .update(cx, |this, cx| {
                                    this.capture_delay.is_none()
                                        && this.preview_active
                                        && this.live_overlay_count(cx) > 0
                                })
                                .map_err(|e| e.to_string())?
                            {
                                break;
                            }
                            if deadline.elapsed().as_secs() > 8 {
                                return Err("countdown did not open region selection".into());
                            }
                        }
                        owner
                            .update(cx, |this, cx| this.close_overlays(cx))
                            .map_err(|e| e.to_string())?;
                        let deadline = std::time::Instant::now();
                        loop {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(20))
                                .await;
                            if owner
                                .update(cx, |this, cx| {
                                    !this.switching_graphics
                                        && !this.graphics
                                        && cx.windows().is_empty()
                                })
                                .map_err(|e| e.to_string())?
                            {
                                break;
                            }
                            if deadline.elapsed().as_secs() > 5 {
                                return Err("cancel did not detach graphics".into());
                            }
                        }
                        Ok(())
                    }
                    .await;
                    *report.lock().unwrap() = Some(checked);
                    cx.update(|cx| cx.quit());
                })
                .detach();
            });
        result
            .lock()
            .unwrap()
            .take()
            .expect("countdown test did not finish")
            .unwrap();
        assert_eq!(errors.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "captures the real desktop and opens/cancels region overlays 101 times; release measurement only"]
    fn resident_region_latency() {
        assert!(
            !cfg!(debug_assertions),
            "run this measurement with --release"
        );
        let errors = track_gui_errors();
        let result = Arc::new(Mutex::new(None));
        let report = result.clone();
        let output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/captures/region-latency-{}",
            probe_timestamp()
        ));
        let destination = output.clone();
        let clipboard =
            unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        gpui_platform::application().with_windowing(gpui::WindowingRequest::Headless).run(move |cx| {
            cx.set_quit_mode(gpui::QuitMode::Explicit);
            let controller = cx.new(|cx| SpikeShell::new_controller(None, cx));
            cx.set_global(ResidentController(controller.clone()));
            controller.update(cx, |this, _| this.backend = XcapBackend::new(destination));
            let owner = controller.downgrade();
            cx.spawn(async move |cx| {
                let measured = async {
                    let deadline = std::time::Instant::now();
                    loop {
                        if !owner.update(cx, |this, _| this.refreshing).map_err(|e| e.to_string())? { break; }
                        if deadline.elapsed().as_secs() > 10 { return Err("metadata timeout".to_string()); }
                        cx.background_executor().timer(std::time::Duration::from_millis(5)).await;
                    }
                    let displays = owner.update(cx, |this, _| this.xcap_displays.clone()).map_err(|e| e.to_string())?;
                    if displays.is_empty() { return Err("no displays available".to_string()); }
                    let mut samples = Vec::new();
                    for cycle in 0..101 {
                        let probe = RegionLatencyProbe { started: std::time::Instant::now(), events: Arc::new(Mutex::new(Vec::new())) };
                        owner.update(cx, |this, cx| {
                            if !this.hotkey.as_ref().is_some_and(|key| key.status().split('|').any(|part| part.trim() == "Ctrl+Shift+R: region")) {
                                return Err("region hotkey is unavailable; no input injected".to_string());
                            }
                            cx.set_global(probe.clone());
                            Ok(())
                        }).map_err(|e| e.to_string())??;
                        send_region_probe_hotkey().map_err(|error| format!("cycle {cycle}: {error}"))?;
                        loop {
                            cx.background_executor().timer(std::time::Duration::from_millis(2)).await;
                            let count = probe.events.lock().unwrap().iter().filter(|(stage, _)| *stage == "overlay_second_callback").count();
                            if count == displays.len() { break; }
                            if probe.started.elapsed().as_secs() > 10 {
                                let status = owner.update(cx, |this, _| this.capture_status.clone()).unwrap_or_default();
                                return Err(format!("region cycle {cycle} timed out: {status}"));
                            }
                        }
                        let compositor_probe = probe.clone();
                        cx.background_spawn(async move {
                            app_core::wait_for_desktop_compositor().map_err(|e| e.to_string())?;
                            compositor_probe.mark("compositor_complete_proxy");
                            Ok::<_, String>(())
                        }).await?;
                        let overlays = owner.update(cx, |this, _| this.overlays.clone()).map_err(|e| e.to_string())?;
                        if overlays.len() != displays.len() { return Err("missing display overlay".to_string()); }
                        for handle in &overlays {
                            let valid = handle.update(cx, |overlay, window, _| {
                                let Some(snapshot) = &overlay.snapshot else { return false; };
                                let viewport = window.viewport_size();
                                overlay.active && overlay.render_image.is_some()
                                    && (f32::from(viewport.width) * window.scale_factor() - snapshot.width as f32).abs() <= 1.0
                                    && (f32::from(viewport.height) * window.scale_factor() - snapshot.height as f32).abs() <= 1.0
                                    && window.bounds().origin == overlay.bounds.origin
                            }).map_err(|e| e.to_string())?;
                            if !valid { return Err(format!("overlay geometry/snapshot mismatch in cycle {cycle}")); }
                        }
                        probe.mark("geometry_checked");
                        samples.push(probe.events.lock().unwrap().clone());
                        // Route cancellation through the rendered overlay's action listener.
                        overlays[0].update(cx, |_, window, cx| window.dispatch_action(Box::new(CloseOverlay), cx)).map_err(|e| e.to_string())?;
                        let deadline = std::time::Instant::now();
                        loop {
                            cx.background_executor().timer(std::time::Duration::from_millis(5)).await;
                            if owner.update(cx, |this, cx| !this.switching_graphics && !this.graphics && cx.windows().is_empty()).map_err(|e| e.to_string())? { break; }
                            if deadline.elapsed().as_secs() > 10 { return Err(format!("cancel action/detach timeout in cycle {cycle}")); }
                        }
                        if unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() } != clipboard {
                            return Err("region cancel changed clipboard (or external clipboard activity)".into());
                        }
                    }
                    Ok((displays, samples))
                }.await;
                *report.lock().unwrap() = Some(measured);
                cx.update(|cx| cx.quit());
            }).detach();
        });
        let (displays, samples) = result
            .lock()
            .unwrap()
            .take()
            .expect("region probe did not finish")
            .unwrap();
        assert_eq!(
            errors.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "GPUI logged region lifecycle errors"
        );
        assert!(!output.exists(), "canceled previews wrote files");
        let displays = displays
            .iter()
            .map(|d| {
                format!(
                    "{{\"width\":{},\"height\":{},\"scale\":{}}}",
                    d.width, d.height, d.scale_factor
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let rows = samples
            .iter()
            .enumerate()
            .map(|(cycle, events)| {
                let events = events
                    .iter()
                    .map(|(stage, ms)| format!("{{\"stage\":\"{stage}\",\"elapsed_ms\":{ms}}}"))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{{\"cycle\":{cycle},\"events\":[{events}]}}")
            })
            .collect::<Vec<_>>()
            .join(",\n");
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/performance/region-latency.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("{{\"scenario\":\"injected native region hotkey to compositor proxy\",\"displays\":[{displays}],\"samples\":[{rows}]}}\n")).unwrap();
        let mut warm: Vec<f64> = samples[1..]
            .iter()
            .map(|events| {
                events
                    .iter()
                    .find(|(stage, _)| *stage == "compositor_complete_proxy")
                    .unwrap()
                    .1
            })
            .collect();
        warm.sort_by(f64::total_cmp);
        println!(
            "region compositor proxy: n=100 p50={:.2} p95={:.2} p99={:.2} max={:.2} ms",
            warm[49], warm[94], warm[98], warm[99]
        );
    }

    #[cfg(target_os = "windows")]
    fn probe_timestamp() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    #[cfg(target_os = "windows")]
    fn send_region_probe_hotkey() -> Result<(), String> {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
        let keys = [VK_CONTROL, VK_SHIFT, 0x52];
        let pressed: Vec<_> = [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN, 0x52]
            .iter()
            .copied()
            .filter(|key| unsafe { GetAsyncKeyState(i32::from(*key)) } < 0)
            .collect();
        if !pressed.is_empty() {
            return Err(format!(
                "release modifier/R keys before running the hotkey probe: {pressed:?}"
            ));
        }
        let event = |key, flags| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: key,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let mut inputs = Vec::new();
        inputs.extend(keys.iter().map(|key| event(*key, 0)));
        inputs.extend(keys.iter().rev().map(|key| event(*key, KEYEVENTF_KEYUP)));
        let sent = unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        };
        if sent != inputs.len() as u32 {
            let releases: Vec<_> = keys
                .iter()
                .rev()
                .map(|key| event(*key, KEYEVENTF_KEYUP))
                .collect();
            unsafe {
                SendInput(
                    releases.len() as u32,
                    releases.as_ptr(),
                    std::mem::size_of::<INPUT>() as i32,
                );
            }
            return Err(format!(
                "native hotkey injection failed after {sent} events"
            ));
        }
        Ok(())
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "opens and closes the real resident control window 101 times; release measurement only"]
    fn resident_control_render_latency() {
        assert!(
            !cfg!(debug_assertions),
            "run this measurement with --release"
        );
        let errors = track_gui_errors();
        let result = Arc::new(Mutex::new(None));
        let report = result.clone();
        gpui_platform::application()
            .with_windowing(gpui::WindowingRequest::Headless)
            .run(move |cx| {
                cx.set_quit_mode(gpui::QuitMode::Explicit);
                let controller = cx.new(|cx| SpikeShell::new_controller(None, cx));
                cx.set_global(ResidentController(controller.clone()));
                let owner = controller.downgrade();
                cx.spawn(async move |cx| {
                    let measured = async {
                        let ready = std::time::Instant::now();
                        loop {
                            if !owner
                                .update(cx, |this, _| this.refreshing)
                                .map_err(|e| e.to_string())?
                            {
                                break;
                            }
                            if ready.elapsed().as_secs() > 10 {
                                return Err("metadata startup timed out".to_string());
                            }
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(5))
                                .await;
                        }
                        let mut samples = Vec::new();
                        for cycle in 0..101 {
                            let frames = Arc::new(Mutex::new(Vec::new()));
                            let observed = frames.clone();
                            owner
                                .update(cx, |this, cx| {
                                    cx.set_global(ControlLatencyProbe {
                                        started: std::time::Instant::now(),
                                        frames_ms: frames,
                                    });
                                    this.show_control(cx);
                                })
                                .map_err(|e| e.to_string())?;
                            let deadline = std::time::Instant::now();
                            loop {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(5))
                                    .await;
                                if observed.lock().unwrap().len() == 2 {
                                    break;
                                }
                                if deadline.elapsed().as_secs() > 10 {
                                    return Err(format!("render timeout in cycle {cycle}"));
                                }
                            }
                            let times = observed.lock().unwrap().clone();
                            samples.push([times[0], times[1]]);
                            owner
                                .update(cx, |this, cx| this.retire_ui(cx))
                                .map_err(|e| e.to_string())?;
                            let deadline = std::time::Instant::now();
                            loop {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(5))
                                    .await;
                                if owner
                                    .update(cx, |this, cx| {
                                        !this.switching_graphics
                                            && !this.graphics
                                            && cx.windows().is_empty()
                                    })
                                    .map_err(|e| e.to_string())?
                                {
                                    break;
                                }
                                if deadline.elapsed().as_secs() > 10 {
                                    return Err(format!("detach timeout in cycle {cycle}"));
                                }
                            }
                        }
                        Ok(samples)
                    }
                    .await;
                    *report.lock().unwrap() = Some(measured);
                    cx.update(|cx| cx.quit());
                })
                .detach();
            });
        let samples = result
            .lock()
            .unwrap()
            .take()
            .expect("probe did not finish")
            .unwrap();
        assert_eq!(
            errors.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "GPUI logged errors during the measured lifecycle"
        );
        let rows = samples
            .iter()
            .enumerate()
            .map(|(cycle, times)| {
                format!(
                    "{{\"cycle\":{cycle},\"first_frame_callback_ms\":{},\"second_frame_callback_ms\":{}}}",
                    times[0], times[1]
                )
            })
            .collect::<Vec<_>>()
            .join(",\n");
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/performance/control-open-latency.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{{\"scenario\":\"resident control reopen after graphics detach\",\"release\":true,\"samples\":[{rows}]}}\n")).unwrap();
        println!(
            "first renderer attach: {:.2} ms to first frame callback",
            samples[0][0]
        );
        for (index, label) in ["first_frame_callback", "second_frame_callback"]
            .iter()
            .enumerate()
        {
            let mut warm: Vec<f64> = samples[1..].iter().map(|times| times[index]).collect();
            warm.sort_by(f64::total_cmp);
            println!(
                "{label}: n=100 p50={:.2} p95={:.2} p99={:.2} max={:.2} ms",
                warm[49], warm[94], warm[98], warm[99]
            );
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "opens real UI, saves a desktop capture and replaces clipboard contents"]
    fn resident_capture_reopen_and_graphics_detach() {
        let errors = track_gui_errors();
        let result = Arc::new(Mutex::new(None));
        let report = result.clone();
        gpui_platform::application().with_windowing(gpui::WindowingRequest::Headless).run(move |cx| {
            cx.set_quit_mode(gpui::QuitMode::Explicit);
            let controller = cx.new(|cx| SpikeShell::new_controller(None, cx));
            cx.set_global(ResidentController(controller.clone()));
            controller.update(cx, |this, _| this.backend = XcapBackend::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/resident-test")));
            let owner = controller.downgrade();
            cx.spawn(async move |cx| {
                let checked = async {
                    let started = std::time::Instant::now();
                    loop {
                        if !owner.update(cx, |this, _| this.refreshing).map_err(|e| e.to_string())? { break; }
                        if started.elapsed().as_secs() > 8 { return Err("initial enumeration timed out".to_string()); }
                        cx.background_executor().timer(std::time::Duration::from_millis(20)).await;
                    }
                    let mut clipboard_sequence = 0;
                    for phase in 0..7 {
                        owner.update(cx, |this, cx| match phase {
                            0 => this.capture_primary(cx),
                            1 | 3 => this.show_control(cx),
                                2 => {
                                    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                                    this.control.unwrap().update(cx, |_, window, _| {
                                        if let RawWindowHandle::Win32(handle) = HasWindowHandle::window_handle(window).unwrap().as_raw() {
                                            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(handle.hwnd.get() as _, 0x0010, 0, 0); }
                                        }
                                    }).unwrap();
                                }
                                4 => this.retire_ui(cx),
                            5 => this.open_overlays(OverlayMode::Fullscreen, cx),
                            6 => this.close_overlays(cx),
                            _ => unreachable!(),
                        }).map_err(|e| e.to_string())?;
                        let started = std::time::Instant::now();
                        loop {
                            cx.background_executor().timer(std::time::Duration::from_millis(20)).await;
                            let ready = owner.update(cx, |this, cx| {
                                if this.switching_graphics || this.capturing || this.copying || this.saving { return false; }
                                match phase {
                                    0 => !this.graphics && this.control.is_none() && cx.windows().is_empty() && this.last_saved.is_some(),
                                    1 | 3 => this.graphics && this.control.is_some(),
                                    5 => this.graphics && this.preview_active && this.live_overlay_count(cx) > 0,
                                    _ => !this.graphics && this.control.is_none() && cx.windows().is_empty(),
                                }
                            }).map_err(|e| e.to_string())?;
                            if ready { break; }
                            if started.elapsed().as_secs() > 10 {
                                let status = owner.update(cx, |this, _| this.capture_status.clone()).unwrap_or_default();
                                return Err(format!("resident phase {phase} timed out: {status}"));
                            }
                        }
                        if phase == 0 { clipboard_sequence = unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() }; }
                        else if unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() } != clipboard_sequence {
                            return Err("UI open/close/cancel changed clipboard".into());
                        }
                    }
                    Ok(())
                }.await;
                *report.lock().unwrap() = Some(checked);
                cx.update(|cx| cx.quit());
            }).detach();
        });
        assert_eq!(
            errors.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "GPUI logged lifecycle errors"
        );
        result
            .lock()
            .unwrap()
            .take()
            .expect("resident test did not finish")
            .unwrap();
    }

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
