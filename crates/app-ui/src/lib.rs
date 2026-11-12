//! Phase 0 spike: GPUI overlay + XCap still capture on Windows.

use gpui_kit as gpui;
use gpui_kit::platform as gpui_platform;
mod caption;
mod assets;
mod workspace;
#[cfg(test)]
mod ui_tests;
#[cfg(all(test, target_os = "linux"))]
mod native_probe;
use workspace::{capture_button, ui_button};
mod redaction_editor;
mod theme;
mod window;
use window::{OpenKitWindow, UiWindow as WindowHandle};
use redaction_editor::{OutputMode, RedactionEditor};

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use app_core::CaptureRequest as CaptureJob;
use app_core::{CaptureBackend, CaptureOutputProfile, CapturedImage, DisplayInfo, WindowInfo, XcapBackend};
use gpui::{
    AnyWindowHandle, App, Bounds, Context, DisplayId, FocusHandle, Focusable, IntoElement,
    KeyBinding, Pixels, TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds,
    WindowKind, WindowOptions, actions, div, img, point, prelude::*, px, rgb, rgba,
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
        NewScreenshot,
        OpenImage,
        PasteImage,
        ShowRecent,
        ShowLibrary,
        ShowSources,
        ShowSettings,
        Quit
    ]
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverlayMode {
    Probe,
    Fullscreen,
}

enum EditorImage {
    Captured(CapturedImage),
    SavedPng(PathBuf),
    File(PathBuf),
    #[cfg(target_os = "windows")]
    Clipboard,
}

#[derive(Clone, Copy)]
enum DelayedCapture {
    Region,
    #[cfg(target_os = "windows")]
    ActiveWindow,
    #[cfg(target_os = "windows")]
    ActiveDisplay,
}

struct ResidentController(gpui::Entity<CaptureWorkspace>);
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

enum UiRequest {
    Control,
    Overlay(OverlayMode),
    Capture(Vec<CaptureJob>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlPage {
    Captures,
    Sources,
    History,
    Uploads,
    Settings,
}

#[derive(Clone, Copy)]
enum HistoryCommand {
    CopyPath,
    #[cfg(target_os = "windows")]
    CopyFile,
    Open,
    Reveal,
    Upload,
    Relink,
    Remove,
}

#[derive(Clone, Copy)]
enum FileClipboardKind {
    Path,
    #[cfg(target_os = "windows")]
    File,
}

pub struct CaptureWorkspace {
    page: ControlPage,
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
    hotkey_bindings: app_core::CaptureHotkeys,
    output_profile: CaptureOutputProfile,
    jpeg_quality: u8,
    export_format: app_core::ImageExportFormat,
    export_size: app_core::ExportSize,
    export_dimension_input: Option<gpui::Entity<gpui::component::input::InputState>>,
    export_height_input: Option<gpui::Entity<gpui::component::input::InputState>>,
    filename_template: String,
    filename_template_input: Option<gpui::Entity<gpui::component::input::InputState>>,
    jpeg_quality_input: Option<gpui::Entity<gpui::component::input::InputState>>,
    last_region: Option<app_core::RememberedRegion>,
    exact_region_inputs: Option<[gpui::Entity<gpui::component::input::InputState>; 4]>,
    exact_region_status: String,
    region_presets: Vec<app_core::RegionPreset>,
    region_preset_name: Option<gpui::Entity<gpui::component::input::InputState>>,
    #[cfg(target_os = "windows")]
    hotkey_inputs: Option<[gpui::Entity<gpui::component::input::InputState>; 6]>,
    history_database: Option<PathBuf>,
    history_visible: bool,
    history_filter: app_core::HistoryFilter,
    history_query: String,
    history_search_input: Option<gpui::Entity<gpui::component::input::InputState>>,
    history_search_subscription: Option<gpui::Subscription>,
    history_revision: u64,
    history_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    history_loading: bool,
    history_mutating: bool,
    history_action_status: String,
    history_refresh_pending: bool,
    history_has_more: bool,
    history_entries: Vec<app_core::HistoryEntry>,
    history_selected: Vec<i64>,
    history_preview: Option<(i64, Arc<gpui::RenderImage>)>,
    history_preview_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    history_preview_status: String,
    history_export_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    upload_destination: Option<app_core::UploadDestination>,
    upload_config_loading: bool,
    upload_profile_checked: bool,
    upload_cancel: Option<app_core::UploadCancellation>,
    upload_generation: u64,
    upload_cancel_requested: bool,
    upload_status: String,
    upload_url: Option<String>,
    upload_history_visible: bool,
    upload_history_loading: bool,
    upload_history_revision: u64,
    upload_history_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    upload_history_entries: Vec<app_core::UploadHistoryEntry>,
    upload_history_more: bool,
    upload_history_status: String,
    history_status: String,
    list_status: String,
    capture_status: String,
    last_capture: Option<CapturedImage>,
    last_saved: Option<PathBuf>,
    last_render: Option<Arc<gpui::RenderImage>>,
    capturing: bool,
    capture_delay: Option<gpui::Task<()>>,
    capture_delay_seconds: u8,
    capture_delay_input: Option<gpui::Entity<gpui::component::input::InputState>>,
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

impl CaptureWorkspace {
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
                            UiRequest::Capture(jobs) => this.start_capture(jobs, cx),
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
        self.restore_upload_destination(cx);
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
                size(px(1060.0), px(760.0)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("screenshots".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(680.), px(480.))),
            ..Default::default()
        };
        self.switching_graphics = true;
        let completion = owner.clone();
        cx.defer(move |cx| {
            let result = cx.open_kit_window(options, move |window, cx| {
                window.on_window_should_close(cx, move |_, cx| {
                    let owner = owner.clone();
                    cx.defer(move |cx| {
                        owner.update(cx, |this, cx| this.retire_ui(cx)).ok();
                    });
                    false
                });
                entity.update(cx, |this, cx| this.init_history_input(window, cx));
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
        if !self.resident
            || self.switching_graphics
            || (self.capturing && self.history_preview_cancel.is_none())
        {
            return;
        }
        self.preview_active = false;
        self.exact_region_inputs = None;
        self.region_preset_name = None;
        self.jpeg_quality_input = None;
        self.filename_template_input = None;
        self.export_dimension_input = None;
        self.export_height_input = None;
        self.capture_delay_input = None;
        self.exact_region_status.clear();
        #[cfg(target_os = "windows")]
        { self.hotkey_inputs = None; }
        self.hide_history(cx);
        self.hide_upload_history();
        if self.page == ControlPage::History {
            self.page = ControlPage::Captures;
        }
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
                                .find(|handle| WindowHandle::<CaptureWorkspace>::from_handle(**handle, cx).is_some())
                                .copied();
                            this.overlays = windows
                                .iter()
                                .filter_map(|handle| WindowHandle::<Overlay>::from_handle(*handle, cx))
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
        Self::with_native_services(control, true, cx)
    }

    #[cfg(test)]
    fn isolated_ui(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_native_services(Some(window.window_handle()), false, cx)
    }

    fn with_native_services(
        control: Option<AnyWindowHandle>,
        native_services: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mac = cfg!(target_os = "macos");
        cx.bind_keys([
            KeyBinding::new(if mac { "cmd-n" } else { "ctrl-n" }, NewScreenshot, Some("CaptureWorkspace")),
            KeyBinding::new(if mac { "cmd-o" } else { "ctrl-o" }, OpenImage, Some("CaptureWorkspace")),
            #[cfg(target_os = "windows")]
            KeyBinding::new("ctrl-shift-v", PasteImage, Some("CaptureWorkspace")),
            KeyBinding::new(if mac { "cmd-1" } else { "ctrl-1" }, ShowRecent, Some("CaptureWorkspace")),
            KeyBinding::new(if mac { "cmd-2" } else { "ctrl-2" }, ShowLibrary, Some("CaptureWorkspace")),
            KeyBinding::new(if mac { "cmd-3" } else { "ctrl-3" }, ShowSources, Some("CaptureWorkspace")),
            KeyBinding::new(if mac { "cmd-," } else { "ctrl-," }, ShowSettings, Some("CaptureWorkspace")),
        ]);
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
            page: ControlPage::Captures,
            diagnostics: false,
            settings_loading: !cfg!(test),
            settings_saving: false,
            output_directory: if cfg!(test) {
                Some(app_core::capture_output_dir())
            } else {
                None
            },
            settings_status: String::new(),
            hotkey_bindings: app_core::CaptureHotkeys::default(),
            output_profile: CaptureOutputProfile::default(),
            jpeg_quality: app_core::DEFAULT_JPEG_QUALITY,
            export_format: app_core::ImageExportFormat::default(),
            export_size: app_core::ExportSize::Original,
            export_dimension_input: None,
            export_height_input: None,
            filename_template: String::new(),
            filename_template_input: None,
            jpeg_quality_input: None,
            last_region: None,
            exact_region_inputs: None,
            exact_region_status: String::new(),
            region_presets: Vec::new(),
            region_preset_name: None,
            #[cfg(target_os = "windows")]
            hotkey_inputs: None,
            history_database: if cfg!(test) {
                None
            } else {
                app_core::history_database_path().ok()
            },
            history_visible: false,
            history_filter: app_core::HistoryFilter::All,
            history_query: String::new(),
            history_search_input: None,
            history_search_subscription: None,
            history_revision: 0,
            history_cancel: None,
            history_loading: false,
            history_mutating: false,
            history_action_status: String::new(),
            history_refresh_pending: false,
            history_has_more: false,
            history_entries: Vec::new(),
            history_selected: Vec::new(),
            history_preview: None,
            history_preview_cancel: None,
            history_preview_status: String::new(),
            history_export_cancel: None,
            upload_destination: None,
            upload_config_loading: false,
            upload_profile_checked: cfg!(test),
            upload_cancel: None,
            upload_generation: 0,
            upload_cancel_requested: false,
            upload_status: "Uploads disabled · Load a destination to enable manual uploads".into(),
            upload_url: None,
            upload_history_visible: false,
            upload_history_loading: false,
            upload_history_revision: 0,
            upload_history_cancel: None,
            upload_history_entries: Vec::new(),
            upload_history_more: false,
            upload_history_status: String::new(),
            history_status: String::new(),
            list_status: String::new(),
            capture_status: "Ready".into(),
            last_capture: None,
            last_saved: None,
            last_render: None,
            capturing: false,
            capture_delay: None,
            capture_delay_seconds: 3,
            capture_delay_input: None,
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
        if native_services {
            shell.refresh_xcap_lists(cx);
        }
        if shell.control.is_some() {
            shell.restore_upload_destination(cx);
        }
        #[cfg(not(test))]
        cx.spawn(async move |this, cx| {
            let settings = cx
                .background_spawn(async { app_core::CaptureSettings::load() })
                .await;
            this.update(cx, |this, cx| {
                this.settings_loading = false;
                match settings {
                    Ok(settings) => {
                        #[cfg(target_os = "windows")]
                        if let Some(hotkey) = this.hotkey.as_mut() {
                            let result = hotkey.register_settings(&settings.hotkeys);
                            this.hotkey_status = hotkey.status().to_string();
                            tracing::info!(status = %this.hotkey_status, "configured global shortcuts ready");
                            if let Err(error) = result {
                                this.settings_status = format!("Saved shortcuts could not be applied: {error:#}. Active: {}", this.hotkey_status);
                            }
                        }
                        this.hotkey_bindings = settings.hotkeys;
                        this.output_profile = settings.output_profile;
                        this.jpeg_quality = settings.jpeg_quality;
                        this.capture_delay_seconds = settings.capture_delay_seconds;
                        this.capture_delay_input = None;
                        this.export_format = settings.export_format;
                        this.filename_template = settings.filename_template;
                        this.region_presets = settings.region_presets;
                        theme::apply(settings.theme, cx);
                        this.backend = XcapBackend::new(&settings.output_directory);
                        this.output_directory = Some(settings.output_directory);
                    }
                    Err(error) => {
                        this.settings_status = format!(
                            "Could not load settings: {error:#}. Please choose a folder."
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
        if native_services {
            match app_core::SpikeTray::new() {
                Ok(tray) => {
                    shell.tray = Some(tray);
                    shell.tray_status =
                        "Tray: capture / active-window countdown / Show control window / Quit".into();
                    tracing::info!("tray icon created");
                }
                Err(error) => {
                    tracing::warn!(%error, "tray creation failed");
                    shell.tray_status = format!("Tray unavailable: {error}");
                }
            }
            {
                let registration = if cfg!(test) {
                    app_core::CaptureHotkey::register()
                } else {
                    app_core::CaptureHotkey::unregistered()
                };
                match registration {
                    Ok(hotkey) => {
                        shell.hotkey_status = hotkey.status().to_string();
                        tracing::info!(status = %shell.hotkey_status, "global hotkey manager ready");
                        shell.hotkey = Some(hotkey);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "global capture hotkey registration failed");
                        shell.hotkey_status = format!("Global hotkeys unavailable: {error}");
                    }
                }
            }
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
                                        Some(app_core::CaptureCommand::ActiveWindow) => this.capture_active_window(cx),
                                        Some(app_core::CaptureCommand::RepeatRegion) => this.repeat_last_region(cx),
                                        Some(app_core::CaptureCommand::ActiveDisplay) => this.capture_active_display(cx),
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
                                        Some(app_core::TrayCommand::ActiveWindow) => this.start_delayed_capture(DelayedCapture::ActiveWindow, cx),
                                        Some(app_core::TrayCommand::ActiveDisplay) => this.start_delayed_capture(DelayedCapture::ActiveDisplay, cx),
                                        Some(app_core::TrayCommand::RepeatRegion) => this.repeat_last_region(cx),
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
        }
        shell
    }

    fn choose_output_directory(&mut self, cx: &mut Context<Self>) {
        let capture_delay_seconds = self.capture_delay_seconds;
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
        let theme = theme::selected(cx);
        let jpeg_quality = self.jpeg_quality;
        let export_format = self.export_format;
        let filename_template = self.filename_template.clone();
        let hotkeys = self.hotkey_bindings.clone();
        let profile = self.output_profile;
        let presets = self.region_presets.clone();
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
                        .map_err(|error| format!("{error:#}"))?.with_theme(theme)
                        .with_hotkeys(hotkeys).map_err(|error| format!("{error:#}"))?
                        .with_output_profile(profile).with_region_presets(presets)
                        .and_then(|settings| settings.with_jpeg_quality(jpeg_quality))
                        .map(|settings| settings.with_export_format(export_format))
                        .and_then(|settings| settings.with_filename_template(filename_template)).and_then(|settings| settings.with_capture_delay(capture_delay_seconds))
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

    fn choose_theme(&mut self, selected: app_core::UiTheme, cx: &mut Context<Self>) {
        let capture_delay_seconds = self.capture_delay_seconds;
        if self.settings_loading || self.settings_saving || self.quitting || theme::selected(cx) == selected {
            return;
        }
        let Some(directory) = self.output_directory.clone() else { return; };
        let previous = theme::selected(cx);
        let jpeg_quality = self.jpeg_quality;
        let export_format = self.export_format;
        let filename_template = self.filename_template.clone();
        let hotkeys = self.hotkey_bindings.clone();
        let profile = self.output_profile;
        let presets = self.region_presets.clone();
        theme::apply(selected, cx);
        self.settings_saving = true;
        self.settings_status = "Saving appearance…".into();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move {
                // The default capture folder may not exist before the first screenshot.
                std::fs::create_dir_all(&directory)?;
                app_core::CaptureSettings::for_directory(directory)?.with_theme(selected).with_hotkeys(hotkeys)?.with_output_profile(profile).with_region_presets(presets)?.with_jpeg_quality(jpeg_quality)?.with_export_format(export_format).with_filename_template(filename_template)?.with_capture_delay(capture_delay_seconds)?.save()
            }).await;
            this.update(cx, |this, cx| {
                this.settings_saving = false;
                match result {
                    Ok(()) => this.settings_status = "Appearance saved".into(),
                    Err(error) => {
                        theme::apply(previous, cx);
                        this.settings_status = format!("Appearance unchanged: {error:#}");
                    }
                }
                this.finish_quit_if_idle(cx);
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }

    fn choose_output_profile(&mut self, selected: CaptureOutputProfile, cx: &mut Context<Self>) {
        let capture_delay_seconds = self.capture_delay_seconds;
        let jpeg_quality = self.jpeg_quality;
        let export_format = self.export_format;
        let filename_template = self.filename_template.clone();
        if self.settings_loading || self.settings_saving || self.quitting || self.capturing
            || self.copying || self.saving || self.preview_active || self.editor.is_some()
            || self.editor_closing || self.switching_graphics || self.capture_delay.is_some()
            || self.output_profile == selected { return; }
        let Some(directory) = self.output_directory.clone() else { return; };
        let appearance = theme::selected(cx);
        let hotkeys = self.hotkey_bindings.clone();
        let presets = self.region_presets.clone();
        self.settings_saving = true;
        self.settings_status = "Saving capture output…".into();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move {
                std::fs::create_dir_all(&directory)?;
                app_core::CaptureSettings::for_directory(directory)?.with_theme(appearance)
                    .with_hotkeys(hotkeys)?.with_output_profile(selected).with_region_presets(presets)?.with_jpeg_quality(jpeg_quality)?.with_export_format(export_format).with_filename_template(filename_template)?.with_capture_delay(capture_delay_seconds)?.save()
            }).await;
            this.update(cx, |this, cx| {
                this.settings_saving = false;
                match result {
                    Ok(()) => {
                        this.output_profile = selected;
                        this.settings_status = "Capture output saved".into();
                    }
                    Err(error) => this.settings_status = format!("Capture output unchanged: {error:#}"),
                }
                this.finish_quit_if_idle(cx);
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }

    fn save_jpeg_quality(&mut self, cx: &mut Context<Self>) {
        let capture_delay_seconds = self.capture_delay_seconds;
        let export_format = self.export_format;
        let filename_template = self.filename_template.clone();
        if self.page != ControlPage::Settings || self.image_input_busy() { return; }
        let Some(input) = &self.jpeg_quality_input else { return; };
        let quality = input.read(cx).value().parse::<u8>().ok().filter(|quality| (1..=100).contains(quality));
        let Some(quality) = quality else {
            self.settings_status = "JPEG quality unchanged: enter a whole number from 1 to 100.".into();
            cx.notify();
            return;
        };
        if quality == self.jpeg_quality {
            self.settings_status = format!("JPEG quality already active: {quality}");
            cx.notify();
            return;
        }
        let Some(directory) = self.output_directory.clone() else { return; };
        let appearance = theme::selected(cx);
        let hotkeys = self.hotkey_bindings.clone();
        let profile = self.output_profile;
        let presets = self.region_presets.clone();
        self.settings_saving = true;
        self.settings_status = "Saving JPEG quality…".into();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move {
                let settings = app_core::CaptureSettings::for_directory(directory)?.with_theme(appearance)
                    .with_hotkeys(hotkeys)?.with_output_profile(profile).with_region_presets(presets)?
                    .with_jpeg_quality(quality)?.with_export_format(export_format).with_filename_template(filename_template)?.with_capture_delay(capture_delay_seconds)?;
                std::fs::create_dir_all(&settings.output_directory)?;
                settings.save()
            }).await;
            this.update(cx, |this, cx| {
                this.settings_saving = false;
                match result {
                    Ok(()) => { this.jpeg_quality = quality; this.settings_status = format!("JPEG quality saved: {quality}"); }
                    Err(error) => this.settings_status = format!("JPEG quality unchanged: {error:#}"),
                }
                this.finish_quit_if_idle(cx);
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }

    fn choose_export_format(&mut self, format: app_core::ImageExportFormat, cx: &mut Context<Self>) {
        let capture_delay_seconds = self.capture_delay_seconds;
        let filename_template = self.filename_template.clone();
        if self.page != ControlPage::Settings || self.image_input_busy() || self.export_format == format { return; }
        let Some(directory) = self.output_directory.clone() else { return; };
        let appearance = theme::selected(cx);
        let hotkeys = self.hotkey_bindings.clone();
        let profile = self.output_profile;
        let presets = self.region_presets.clone();
        let quality = self.jpeg_quality;
        self.settings_saving = true;
        self.settings_status = "Saving default export format…".into();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move {
                let settings = app_core::CaptureSettings::for_directory(directory)?.with_theme(appearance)
                    .with_hotkeys(hotkeys)?.with_output_profile(profile).with_region_presets(presets)?
                    .with_jpeg_quality(quality)?.with_export_format(format).with_filename_template(filename_template)?.with_capture_delay(capture_delay_seconds)?;
                std::fs::create_dir_all(&settings.output_directory)?;
                settings.save()
            }).await;
            this.update(cx, |this, cx| {
                this.settings_saving = false;
                match result {
                    Ok(()) => { this.export_format = format; this.settings_status = format!("Default export format saved: {}", format.label()); }
                    Err(error) => this.settings_status = format!("Default export format unchanged: {error:#}"),
                }
                this.finish_quit_if_idle(cx);
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }

    fn save_filename_template(&mut self, cx: &mut Context<Self>) {
        let capture_delay_seconds = self.capture_delay_seconds;
        if self.page != ControlPage::Settings || self.image_input_busy() { return; }
        let Some(input) = &self.filename_template_input else { return; };
        let template = input.read(cx).value().trim().to_owned();
        if let Err(error) = app_core::validate_capture_name_template(&template) {
            self.settings_status = format!("Filename template unchanged: {error}");
            cx.notify(); return;
        }
        if template == self.filename_template {
            self.settings_status = "Filename template already active.".into();
            cx.notify(); return;
        }
        let Some(directory) = self.output_directory.clone() else { return; };
        let appearance = theme::selected(cx);
        let hotkeys = self.hotkey_bindings.clone();
        let profile = self.output_profile;
        let presets = self.region_presets.clone();
        let quality = self.jpeg_quality;
        let format = self.export_format;
        self.settings_saving = true;
        self.settings_status = "Saving filename template…".into();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move {
                let settings = app_core::CaptureSettings::for_directory(directory)?.with_theme(appearance)
                    .with_hotkeys(hotkeys)?.with_output_profile(profile).with_region_presets(presets)?
                    .with_jpeg_quality(quality)?.with_export_format(format).with_filename_template(template)?.with_capture_delay(capture_delay_seconds)?;
                std::fs::create_dir_all(&settings.output_directory)?;
                settings.save()?;
                Ok::<_, anyhow::Error>(settings.filename_template)
            }).await;
            this.update(cx, |this, cx| {
                this.settings_saving = false;
                match result {
                    Ok(template) => { this.filename_template = template; this.settings_status = "Filename template saved.".into(); }
                    Err(error) => this.settings_status = format!("Filename template unchanged: {error:#}"),
                }
                this.finish_quit_if_idle(cx);
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }

    fn show_page(&mut self, page: ControlPage, cx: &mut Context<Self>) {
        if self.page == page && (page != ControlPage::History || self.history_visible) {
            return;
        }
        self.hide_history(cx);
        self.hide_upload_history();
        self.page = page;
        self.capture_delay_input = None;
        self.export_dimension_input = None;
        self.export_height_input = None;
        self.jpeg_quality_input = None;
        self.filename_template_input = None;
        self.exact_region_inputs = None;
        self.region_preset_name = None;
        self.exact_region_status.clear();
        #[cfg(target_os = "windows")]
        { self.hotkey_inputs = None; }
        if page == ControlPage::History {
            self.history_visible = true;
            self.refresh_history(None, cx);
            if let Some(control) = self.control {
                let owner = cx.entity().downgrade();
                cx.defer(move |cx| {
                    control.update(cx, |_, window, cx| {
                        owner.update(cx, |this, cx| this.init_history_input(window, cx)).ok();
                    }).ok();
                });
            }
        }
        cx.notify();
    }
    #[cfg(target_os = "windows")]
    fn save_hotkeys(&mut self, bindings: app_core::CaptureHotkeys, cx: &mut Context<Self>) {
        let capture_delay_seconds = self.capture_delay_seconds;
        let jpeg_quality = self.jpeg_quality;
        let export_format = self.export_format;
        let filename_template = self.filename_template.clone();
        if self.page != ControlPage::Settings || self.settings_loading || self.settings_saving || self.quitting {
            return;
        }
        let Some(directory) = self.output_directory.clone() else { return; };
        let bindings = match bindings.validated() {
            Ok(bindings) => bindings,
            Err(error) => {
                self.settings_status = format!("Shortcuts unchanged: {error:#}");
                cx.notify();
                return;
            }
        };
        let Some(hotkey) = self.hotkey.as_mut() else { return; };
        if let Err(error) = hotkey.prepare(&bindings) {
            self.hotkey_status = hotkey.status().to_string();
            self.settings_status = format!("Shortcuts unchanged: {error:#}");
            cx.notify();
            return;
        }
        let appearance = theme::selected(cx);
        let profile = self.output_profile;
        self.settings_saving = true;
        self.settings_status = "Saving shortcuts…".into();
        let saved_bindings = bindings.clone();
        let presets = self.region_presets.clone();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move {
                std::fs::create_dir_all(&directory)?;
                app_core::CaptureSettings::for_directory(directory)?
                    .with_theme(appearance).with_hotkeys(saved_bindings)?.with_output_profile(profile).with_region_presets(presets)?.with_jpeg_quality(jpeg_quality)?.with_export_format(export_format).with_filename_template(filename_template)?.with_capture_delay(capture_delay_seconds)?.save()
            }).await;
            this.update(cx, |this, cx| {
                this.settings_saving = false;
                if result.is_ok() { this.hotkey_bindings = bindings; }
                let cleanup = this.hotkey.as_mut().unwrap().finish(result.is_ok());
                this.hotkey_status = this.hotkey.as_ref().unwrap().status().to_string();
                this.settings_status = match (result, cleanup) {
                    (Ok(()), Ok(())) => "Shortcuts saved and active".into(),
                    (Err(error), Ok(())) => format!("Shortcuts unchanged; settings were not saved: {error:#}"),
                    (save, Err(error)) => format!("Shortcut cleanup failed: {error:#}. Settings saved: {}. Active bindings are listed below; restart to retry cleanup.", save.is_ok()),
                };
                // Rebuild drafts only if the settings page is still mounted.
                this.hotkey_inputs = None;
                this.finish_quit_if_idle(cx);
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }

    fn init_history_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui::component::input::{InputEvent, InputState};
        if !self.history_visible || self.history_search_input.is_some() {
            return;
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Search images by name, date or source…")
                .clean_on_escape()
                .validate(|value, _| workspace::valid_search(value))
        });
        self.history_search_subscription = Some(cx.subscribe_in(
            &input, window, |this, field, event, _, cx| {
                if matches!(event, InputEvent::Change)
                    && this.history_search_input.as_ref().is_some_and(|current| {
                        current.entity_id() == field.entity_id()
                    })
                {
                    this.set_history_query(field.read(cx).value().to_string(), cx);
                }
            },
        ));
        self.history_search_input = Some(input);
        cx.notify();
    }
    fn hide_history(&mut self, cx: &mut Context<Self>) {
        self.clear_history_preview(cx);
        self.history_visible = false;
        self.history_query.clear();
        self.history_search_input = None;
        self.history_search_subscription = None;
        self.history_revision = self.history_revision.wrapping_add(1);
        if let Some(cancel) = &self.history_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.history_refresh_pending = false;
        self.history_entries.clear();
        self.history_selected.clear();
        self.history_has_more = false;
        self.history_status.clear();
        self.history_action_status.clear();
    }
    fn set_history_filter(&mut self, filter: app_core::HistoryFilter, cx: &mut Context<Self>) {
        if self.history_filter == filter || self.history_mutating {
            return;
        }
        self.history_filter = filter;
        self.restart_history(cx);
    }
    fn run_history_command(
        &mut self,
        id: i64,
        revision: u64,
        command: HistoryCommand,
        cx: &mut Context<Self>,
    ) {
        if !self.history_visible || self.history_revision != revision || self.history_mutating {
            return;
        }
        let Some(entry) = self.history_entries.iter().find(|entry| entry.id == id) else {
            return;
        };
        let path = entry.path.clone();
        match command {
            HistoryCommand::Open => cx.open_with_system(&path),
            HistoryCommand::Reveal => cx.reveal_path(&path),
            HistoryCommand::CopyPath => {
                if !self.history_loading { self.copy_file_reference(path, Some(revision), FileClipboardKind::Path, cx); }
            }
            #[cfg(target_os = "windows")]
            HistoryCommand::CopyFile => {
                if !self.history_loading { self.copy_file_reference(path, Some(revision), FileClipboardKind::File, cx); }
            }
            HistoryCommand::Upload => self.upload_history_image(id, revision, cx),
            HistoryCommand::Relink => self.relink_history_image(id, revision, cx),
            HistoryCommand::Remove => self.remove_history_capture(id, cx),
        }
    }
    fn set_history_query(&mut self, query: String, cx: &mut Context<Self>) {
        if !self.history_visible || self.history_query == query || query.len() > 512 {
            return;
        }
        self.history_query = query;
        self.restart_history(cx);
    }
    fn restart_history(&mut self, cx: &mut Context<Self>) {
        self.clear_history_preview(cx);
        self.history_revision = self.history_revision.wrapping_add(1);
        if let Some(cancel) = &self.history_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.history_entries.clear();
        self.history_selected.clear();
        self.history_has_more = false;
        self.refresh_history(None, cx);
        cx.notify();
    }
    fn refresh_history(&mut self, before: Option<i64>, cx: &mut Context<Self>) {
        if self.quitting || !self.history_visible {
            return;
        }
        self.history_status = "Loading history…".into();
        if self.history_loading || self.history_mutating {
            self.history_refresh_pending = true;
            return;
        }
        let Some(database) = self.history_database.clone() else {
            self.history_status = "History storage is unavailable".into();
            cx.notify();
            return;
        };
        self.history_selected.clear();
        self.history_revision = self.history_revision.wrapping_add(1);
        self.clear_history_preview(cx);
        self.history_loading = true;
        self.history_refresh_pending = false;
        let filter = self.history_filter;
        let query = self.history_query.clone();
        let revision = self.history_revision;
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.history_cancel = Some(cancel.clone());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    app_core::search_capture_history(&database, before, filter, &query, cancel)
                })
                .await;
            this.update(cx, |this, cx| {
                this.history_loading = false;
                this.history_cancel = None;
                if this.quitting {
                    return;
                }
                if this.history_visible && this.history_revision == revision {
                    match result {
                        Ok(mut entries) => {
                            this.history_has_more = entries.len() > 20;
                            entries.truncate(20);
                            this.history_entries = entries;
                            this.history_status = if !this.history_query.trim().is_empty() {
                                if this.history_entries.is_empty() {
                                    "No matching history entries"
                                } else {
                                    "Search results · newest first"
                                }
                                .into()
                            } else if this.history_entries.is_empty() {
                                match filter {
                                    app_core::HistoryFilter::All => "No saved captures yet",
                                    app_core::HistoryFilter::Captures => {
                                        "No app captures in history"
                                    }
                                    app_core::HistoryFilter::Imports => {
                                        "No imported images in history"
                                    }
                                }
                                .into()
                            } else {
                                match filter {
                                    app_core::HistoryFilter::All => "Saved captures · newest first",
                                    app_core::HistoryFilter::Captures => {
                                        "App captures · newest first"
                                    }
                                    app_core::HistoryFilter::Imports => {
                                        "Imported images · newest first"
                                    }
                                }
                                .into()
                            };
                        }
                        Err(error) => {
                            this.history_status = format!("History could not be loaded: {error:#}")
                        }
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
            match handle.update(cx, |_overlay, window, _cx| {
                #[cfg(target_os = "windows")]
                if let Some(native) = &_overlay.native {
                    if let Err(error) = native.request_visible(false) {
                        tracing::error!(%error, "could not hide overlay");
                        return;
                    }
                    _overlay.active = false;
                    _overlay.drag_start = None;
                    _overlay.snapshot = None;
                    if let Some(render) = _overlay.render_image.take() {
                        if let Err(error) = window.drop_image(render) {
                            tracing::warn!(%error, "could not release overlay image atlas entry");
                        }
                    }
                    _cx.notify();
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
        self.start_delayed_capture(DelayedCapture::Region, cx);
    }

    fn apply_capture_delay(&mut self, cx: &mut Context<Self>) {
        if self.page != ControlPage::Captures || self.image_input_busy() { return; }
        let seconds = self.capture_delay_input.as_ref().and_then(|input| input.read(cx).value().parse::<u8>().ok());
        let Some(seconds) = seconds.filter(|seconds| (1..=60).contains(seconds)) else {
            self.capture_status = "Delay unchanged: enter whole seconds from 1 to 60.".into();
            cx.notify(); return;
        };
        if seconds == self.capture_delay_seconds { return; }
        let Some(directory) = self.output_directory.clone() else { return; };
        let appearance = theme::selected(cx);
        let hotkeys = self.hotkey_bindings.clone();
        let profile = self.output_profile;
        let presets = self.region_presets.clone();
        let quality = self.jpeg_quality;
        let format = self.export_format;
        let template = self.filename_template.clone();
        self.settings_saving = true;
        self.capture_status = "Saving capture delay…".into();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move {
                let settings = app_core::CaptureSettings::for_directory(directory)?.with_theme(appearance)
                    .with_hotkeys(hotkeys)?.with_output_profile(profile).with_region_presets(presets)?
                    .with_jpeg_quality(quality)?.with_export_format(format).with_filename_template(template)?
                    .with_capture_delay(seconds)?;
                std::fs::create_dir_all(&settings.output_directory)?;
                settings.save()
            }).await;
            this.update(cx, |this, cx| {
                this.settings_saving = false;
                this.capture_status = match result {
                    Ok(()) => { this.capture_delay_seconds = seconds; format!("Capture delay saved: {seconds} s.") }
                    Err(error) => format!("Capture delay unchanged: {error:#}"),
                };
                this.finish_quit_if_idle(cx);
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }

    fn start_delayed_capture(&mut self, target: DelayedCapture, cx: &mut Context<Self>) {
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
        let label = match target { DelayedCapture::Region => "Region", #[cfg(target_os = "windows")] DelayedCapture::ActiveWindow => "Active window", #[cfg(target_os = "windows")] DelayedCapture::ActiveDisplay => "Active display" };
        let instruction = match target { DelayedCapture::Region => "Press Escape to cancel.", #[cfg(target_os = "windows")] DelayedCapture::ActiveWindow | DelayedCapture::ActiveDisplay => "Focus the target window. Escape in this app cancels." };
        let seconds = self.capture_delay_seconds;
        self.capture_status = format!("{label} capture in {seconds} s… {instruction}");
        self.capture_delay = Some(cx.spawn(async move |this, cx| {
            for remaining in (0..seconds).rev() {
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
                            match target {
                                DelayedCapture::Region => this.open_overlays(OverlayMode::Fullscreen, cx),
                                #[cfg(target_os = "windows")]
                                DelayedCapture::ActiveWindow => this.capture_active_window(cx),
                                #[cfg(target_os = "windows")]
                                DelayedCapture::ActiveDisplay => this.capture_active_display(cx),
                            }
                        } else {
                            this.capture_status = format!(
                                "{label} capture in {remaining} {}… {instruction}",
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
        let focus = self.focus_handle.clone();
        cx.defer(move |cx| {
            let result = control
                .update(cx, |_, window, cx| {
                    window.focus(&focus, cx);
                    show_control_window(window)
                })
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

            match cx.open_kit_window(
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

    #[cfg(target_os = "windows")]
    fn capture_active_window(&mut self, cx: &mut Context<Self>) {
        if self.capturing || self.copying || self.saving || self.quitting || self.settings_loading
            || self.settings_saving || self.switching_graphics || self.preview_active
            || self.editor.is_some() || self.editor_closing || self.capture_delay.is_some()
            || self.output_directory.is_none() { return; }
        match app_core::foreground_window_capture() {
            Ok(request) => self.start_capture(vec![request], cx),
            Err(error) => {
                self.capture_status = format!("Active window not captured: {error}");
                self.show_control(cx);
                cx.notify();
            }
        }
    }

    #[cfg(target_os = "windows")]
    fn capture_active_display(&mut self, cx: &mut Context<Self>) {
        if self.image_input_busy() { return; }
        match app_core::foreground_display_capture() {
            Ok(request) => self.start_capture(vec![request], cx),
            Err(error) => {
                self.capture_status = format!("Active display not captured: {error}");
                self.show_control(cx);
                cx.notify();
            }
        }
    }

    fn set_last_region(&mut self, region: Option<app_core::RememberedRegion>) {
        self.last_region = region;
        #[cfg(target_os = "windows")]
        if let Some(tray) = &self.tray { tray.set_repeat_region_available(self.last_region.is_some()); }
    }

    fn repeat_last_region(&mut self, cx: &mut Context<Self>) {
        if self.capturing || self.copying || self.saving || self.quitting || self.settings_loading
            || self.settings_saving || self.switching_graphics || self.preview_active
            || self.editor.is_some() || self.editor_closing || self.capture_delay.is_some()
            || self.output_directory.is_none() { return; }
        let Some(region) = self.last_region.clone() else {
            self.capture_status = "Select a region on a stable display before repeating it. Regions are remembered for this app session only.".into();
            self.show_page(ControlPage::Captures, cx);
            self.show_control(cx);
            cx.notify();
            return;
        };
        self.start_capture(vec![CaptureJob::RepeatRegion(region)], cx);
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

    fn capture_exact_region(&mut self, display: DisplayInfo, cx: &mut Context<Self>) {
        if self.page != ControlPage::Sources || self.image_input_busy() || self.refreshing
            || !self.backend.capabilities().capture_region { return; }
        let Some(inputs) = &self.exact_region_inputs else { return; };
        let values = std::array::from_fn(|index| inputs[index].read(cx).value().to_string());
        match workspace::exact_region_request(&display, &values) {
            Ok(region) => {
                self.exact_region_status.clear();
                self.show_page(ControlPage::Captures, cx);
                self.start_capture(vec![CaptureJob::ExactRegion(region)], cx);
            }
            Err(error) => { self.exact_region_status = error; cx.notify(); }
        }
    }

    fn save_region_preset(&mut self, display: DisplayInfo, cx: &mut Context<Self>) {
        if self.page != ControlPage::Sources || self.image_input_busy() || self.refreshing { return; }
        let (Some(inputs), Some(name)) = (&self.exact_region_inputs, &self.region_preset_name) else { return; };
        let values = std::array::from_fn(|index| inputs[index].read(cx).value().to_string());
        let result = workspace::exact_region_request(&display, &values)
            .and_then(|region| app_core::RegionPreset::new(&name.read(cx).value(), region.area()).map_err(|e| e.to_string()));
        match result {
            Ok(preset) => {
                let mut presets = self.region_presets.clone();
                if let Some(previous) = presets.iter_mut().find(|old| old.name == preset.name) { *previous = preset; }
                else { presets.push(preset); }
                self.persist_region_presets(presets, cx);
            }
            Err(error) => { self.exact_region_status = error; cx.notify(); }
        }
    }

    fn use_region_preset(&mut self, preset: &app_core::RegionPreset, window: &mut Window, cx: &mut Context<Self>) {
        if self.page != ControlPage::Sources || self.image_input_busy() || !self.region_presets.contains(preset) { return; }
        let Some(inputs) = &self.exact_region_inputs else { return; };
        for (input, value) in inputs.iter().zip([preset.x, preset.y, preset.width, preset.height]) {
            input.update(cx, |input, cx| input.set_value(value.to_string(), window, cx));
        }
        if let Some(name) = &self.region_preset_name {
            name.update(cx, |input, cx| input.set_value(preset.name.clone(), window, cx));
        }
        self.exact_region_status = "Preset loaded. Choose a display and capture explicitly.".into();
        cx.notify();
    }

    fn remove_region_preset(&mut self, preset: &app_core::RegionPreset, cx: &mut Context<Self>) {
        if self.page != ControlPage::Sources || self.image_input_busy() { return; }
        let Some(index) = self.region_presets.iter().position(|current| current == preset) else { return; };
        let mut presets = self.region_presets.clone();
        presets.remove(index);
        self.persist_region_presets(presets, cx);
    }

    fn persist_region_presets(&mut self, presets: Vec<app_core::RegionPreset>, cx: &mut Context<Self>) {
        let capture_delay_seconds = self.capture_delay_seconds;
        let jpeg_quality = self.jpeg_quality;
        let export_format = self.export_format;
        let filename_template = self.filename_template.clone();
        let Some(directory) = self.output_directory.clone() else { return; };
        let appearance = theme::selected(cx);
        let hotkeys = self.hotkey_bindings.clone();
        let profile = self.output_profile;
        self.settings_saving = true;
        self.exact_region_status = "Saving region presets…".into();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move {
                let settings = app_core::CaptureSettings::for_directory(directory)?.with_theme(appearance)
                    .with_hotkeys(hotkeys)?.with_output_profile(profile).with_region_presets(presets)?.with_jpeg_quality(jpeg_quality)?.with_export_format(export_format).with_filename_template(filename_template)?.with_capture_delay(capture_delay_seconds)?;
                std::fs::create_dir_all(&settings.output_directory)?;
                settings.save()?;
                Ok::<_, anyhow::Error>(settings.region_presets)
            }).await;
            this.update(cx, |this, cx| {
                this.settings_saving = false;
                match result {
                    Ok(presets) => { this.region_presets = presets; this.exact_region_status = "Region presets saved.".into(); }
                    Err(error) => this.exact_region_status = format!("Region presets unchanged: {error:#}"),
                }
                this.settings_status = this.exact_region_status.clone();
                this.finish_quit_if_idle(cx);
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
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
            && !self.upload_config_loading
            && self.upload_cancel.is_none()
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
        let source = self.last_capture.clone().map(EditorImage::Captured)
            .or_else(|| self.last_saved.clone().map(EditorImage::SavedPng));
        if let Some(source) = source { self.open_redaction_source(source, cx); }
    }

    fn open_history_editor(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.open_redaction_source(EditorImage::SavedPng(path), cx);
    }

    fn image_input_busy(&self) -> bool {
        self.editor.is_some() || self.editor_closing || self.capturing || self.copying
            || self.saving || self.quitting || self.capture_delay.is_some()
            || self.settings_loading || self.settings_saving || self.history_mutating
            || self.switching_graphics || self.preview_active || self.output_directory.is_none()
    }

    fn paste_image(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "windows")]
        if !self.image_input_busy() {
            self.show_page(ControlPage::Captures, cx);
            self.open_redaction_source(EditorImage::Clipboard, cx);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = cx;
    }

    fn open_image(&mut self, cx: &mut Context<Self>) {
        if self.image_input_busy() { return; }
        self.show_page(ControlPage::Captures, cx);
        self.capturing = true;
        self.capture_status = "Choosing an image…".into();
        let dialog = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open a PNG, JPEG or WebP image to edit a copy".into()),
        });
        cx.spawn(async move |this, cx| {
            let selected = dialog.await;
            this.update(cx, |this, cx| {
                this.capturing = false;
                if this.quitting {
                    this.finish_quit_if_idle(cx);
                    return;
                }
                match selected {
                    Ok(Ok(Some(paths))) if paths.len() == 1 => {
                        this.open_redaction_source(EditorImage::File(paths.into_iter().next().unwrap()), cx);
                    }
                    Ok(Ok(None)) => this.capture_status = "Image selection canceled.".into(),
                    Ok(Ok(Some(_))) => this.capture_status = "Choose exactly one PNG, JPEG or WebP image.".into(),
                    Err(_) | Ok(Err(_)) => this.capture_status = "Could not select an image.".into(),
                }
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }

    fn open_dropped_image(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        if self.image_input_busy() { return; }
        self.show_page(ControlPage::Captures, cx);
        let [path] = paths else {
            self.capture_status = "Drop exactly one PNG, JPEG or WebP image to edit a copy.".into();
            cx.notify();
            return;
        };
        self.open_redaction_source(EditorImage::File(path.clone()), cx);
    }

    fn open_redaction_source(
        &mut self,
        source: EditorImage,
        cx: &mut Context<Self>,
    ) {
        if self.image_input_busy() {
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
                        EditorImage::Captured(source) => source,
                        file => {
                            let pixels = match file {
                                EditorImage::SavedPng(path) => app_core::load_saved_png(&path),
                                EditorImage::File(path) => app_core::load_editor_image(&path),
                                #[cfg(target_os = "windows")]
                                EditorImage::Clipboard => app_core::read_clipboard_image(),
                                EditorImage::Captured(_) => unreachable!(),
                            }.map_err(|e| format!("{e:#}"))?;
                            CapturedImage {
                                width: pixels.width(),
                                height: pixels.height(),
                                pixels: Arc::new(pixels),
                                display_id: "saved-image".into(),
                                window_id: None,
                                png_path: directory.join("source.png"),
                                elapsed_ms: 0,
                                source_format: app_core::PixelFormat::Rgba8UnormSrgb,
                                display_geometry: None,
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
        match cx.open_kit_window(options, |window, cx| {
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
        let remembered = crop.and_then(|area| app_core::RememberedRegion::from_snapshot(&source, area).ok());
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
                        if let Some(region) = remembered { this.set_last_region(Some(region)); }
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

    fn clear_history_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.history_preview_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.history_preview_status.clear();
        if let Some((_, render)) = self.history_preview.take() {
            if let Some(control) = self.control {
                cx.defer(move |cx| {
                    control
                        .update(cx, |_, window, _| {
                            window.drop_image(render).ok();
                        })
                        .ok();
                });
            }
        }
    }

    fn preview_history_image(&mut self, id: i64, revision: u64, cx: &mut Context<Self>) {
        if self.quitting || !self.history_visible || self.history_revision != revision {
            return;
        }
        if self
            .history_preview
            .as_ref()
            .is_some_and(|(current, _)| *current == id)
        {
            self.clear_history_preview(cx);
            cx.notify();
            return;
        }
        if self.capturing
            || self.copying
            || self.saving
            || self.history_loading
            || self.history_mutating
            || self.history_preview_cancel.is_some()
            || self.capture_delay.is_some()
            || self.editor.is_some()
            || self.editor_closing
        {
            return;
        }
        let Some(path) = self
            .history_entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.path.clone())
        else {
            return;
        };
        self.clear_history_preview(cx);
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.history_preview_cancel = Some(cancel.clone());
        self.history_preview_status = "Loading preview…".into();
        self.capturing = true;
        cx.spawn(async move |this, cx| {
            let flag = cancel.clone();
            let result = cx
                .background_spawn(async move {
                    app_core::load_png_thumbnail(&path, || {
                        flag.load(std::sync::atomic::Ordering::Relaxed)
                    })
                    .map(|mut pixels| {
                        for pixel in pixels.pixels_mut() {
                            pixel.0.swap(0, 2);
                        }
                        Arc::new(gpui::RenderImage::new(vec![image::Frame::new(pixels)]))
                    })
                    .map_err(|error| format!("{error:#}"))
                })
                .await;
            this.update(cx, |this, cx| {
                this.capturing = false;
                this.history_preview_cancel = None;
                if !this.quitting
                    && this.history_visible
                    && this.history_revision == revision
                    && !cancel.load(std::sync::atomic::Ordering::Relaxed)
                {
                    match result {
                        Ok(render) => {
                            this.history_preview = Some((id, render));
                            this.history_preview_status =
                                "Preview only · Image and clipboard unchanged".into();
                        }
                        Err(error) => {
                            this.history_preview_status = format!("Preview unavailable: {error}")
                        }
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

    fn copy_last_file_path(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.last_saved.clone() {
            self.copy_file_reference(path, None, FileClipboardKind::Path, cx);
        }
    }

    #[cfg(target_os = "windows")]
    fn copy_last_file(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.last_saved.clone() {
            self.copy_file_reference(path, None, FileClipboardKind::File, cx);
        }
    }

    fn copy_file_reference(&mut self, path: PathBuf, history_revision: Option<u64>, kind: FileClipboardKind, cx: &mut Context<Self>) {
        if self.capturing || self.capture_delay.is_some() || self.copying || self.saving
            || self.quitting || self.settings_loading || self.settings_saving || self.history_mutating
            || self.switching_graphics || self.preview_active || self.editor.is_some() || self.editor_closing { return; }
        let (label, success) = match kind {
            FileClipboardKind::Path => ("file path", "File path copied to clipboard"),
            #[cfg(target_os = "windows")]
            FileClipboardKind::File => ("file", "File copied to clipboard"),
        };
        let clipboard = self.clipboard.clone();
        self.copying = true;
        self.clipboard_status = format!("Copying {label}…");
        if history_revision.is_some() { self.history_action_status = self.clipboard_status.clone(); }
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move {
                let mut clipboard = clipboard.lock().map_err(|_| anyhow::anyhow!("Clipboard state is unavailable"))?;
                match kind {
                    FileClipboardKind::Path => {
                        let text = path.to_str().ok_or_else(|| anyhow::anyhow!("The path cannot be represented as Unicode text"))?;
                        clipboard.copy_text(text.to_owned())
                    }
                    #[cfg(target_os = "windows")]
                    FileClipboardKind::File => clipboard.copy_file(&path),
                }
            }).await;
            this.update(cx, |this, cx| {
                this.copying = false;
                this.clipboard_status = match result {
                    Ok(()) => success.into(),
                    Err(error) => format!("Could not copy {label}: {error:#}"),
                };
                if history_revision.is_some_and(|revision| this.history_visible && this.history_revision == revision) {
                    this.history_action_status = this.clipboard_status.clone();
                }
                this.finish_quit_if_idle(cx);
                cx.notify();
            }).ok();
        }).detach();
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

    fn toggle_history_selection(&mut self, id: i64, revision: u64, cx: &mut Context<Self>) {
        if !self.history_visible
            || self.history_loading
            || self.history_mutating
            || self.history_revision != revision
            || !self.history_entries.iter().any(|entry| entry.id == id)
        {
            return;
        }
        if let Some(index) = self
            .history_selected
            .iter()
            .position(|selected| *selected == id)
        {
            self.history_selected.remove(index);
        } else if self.history_selected.len() < 20 {
            self.history_selected.push(id);
        }
        cx.notify();
    }

    fn select_history_page(&mut self, revision: u64, cx: &mut Context<Self>) {
        if self.history_visible
            && !self.history_loading
            && !self.history_mutating
            && self.history_revision == revision
        {
            self.history_selected = self
                .history_entries
                .iter()
                .take(20)
                .map(|entry| entry.id)
                .collect();
            cx.notify();
        }
    }

    fn finish_history_mutation(&mut self, refresh: bool, cx: &mut Context<Self>) {
        self.history_mutating = false;
        let pending = std::mem::take(&mut self.history_refresh_pending);
        if refresh || pending {
            self.refresh_history(None, cx);
        }
        self.finish_quit_if_idle(cx);
        cx.notify();
    }

    fn remove_history_capture(&mut self, id: i64, cx: &mut Context<Self>) {
        self.remove_history_captures(vec![id], cx);
    }

    fn remove_history_captures(&mut self, ids: Vec<i64>, cx: &mut Context<Self>) {
        if self.quitting
            || !self.history_visible
            || self.history_loading
            || self.history_mutating
            || self.capturing
            || self.copying
            || self.saving
            || self.capture_delay.is_some()
            || ids.is_empty()
            || ids.len() > 20
            || ids
                .iter()
                .any(|id| !self.history_entries.iter().any(|entry| entry.id == *id))
        {
            return;
        }
        let Some(database) = self.history_database.clone() else {
            return;
        };
        self.history_mutating = true;
        self.history_action_status = "Removing history entries…".into();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { app_core::remove_history_entries(&database, &ids) })
                .await;
            this.update(cx, |this, cx| {
                let refresh = result.is_ok();
                this.history_action_status = match result {
                    Ok(removed) => format!("Removed {removed} history entries. Image files kept."),
                    Err(error) => format!("Could not remove entries: {error:#}"),
                };
                this.finish_history_mutation(refresh, cx);
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
                        this.history_action_status = format!("Image selection failed: {error:?}");
                        this.finish_history_mutation(false, cx);
                    })
                    .ok();
                    return;
                }
            };
            let Some(paths) = selected else {
                this.update(cx, |this, cx| {
                    this.finish_history_mutation(false, cx);
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
                let refresh = result.is_ok();
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
                    }
                    Err(error) => this.history_action_status = format!("Import failed: {error:#}"),
                }
                this.finish_history_mutation(refresh, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn relink_history_image(&mut self, id: i64, revision: u64, cx: &mut Context<Self>) {
        if self.quitting
            || !self.history_visible
            || self.history_revision != revision
            || self.history_loading
            || self.history_mutating
            || self.capturing
            || self.copying
            || self.saving
            || self.settings_saving
            || self.capture_delay.is_some()
        {
            return;
        }
        let Some(entry) = self
            .history_entries
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
        else {
            return;
        };
        let Some(database) = self.history_database.clone() else {
            return;
        };
        self.history_mutating = true;
        self.history_action_status =
            "Choose the PNG for this history entry. Image files stay unchanged.".into();
        let dialog = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Relink history entry to a PNG image".into()),
        });
        cx.spawn(async move |this, cx| {
            let selection = dialog.await;
            let path = this
                .update(cx, |this, cx| {
                    if this.quitting || !this.history_visible || this.history_revision != revision {
                        this.finish_history_mutation(false, cx);
                        return None;
                    }
                    match selection {
                        Ok(Ok(Some(paths))) if paths.len() == 1 => {
                            this.history_action_status = "Relinking image…".into();
                            cx.notify();
                            paths.into_iter().next()
                        }
                        other => {
                            this.history_action_status = match other {
                                Ok(Ok(None)) => "Relink canceled. History unchanged.".into(),
                                Ok(Ok(Some(_))) => {
                                    "Select exactly one PNG. History unchanged.".into()
                                }
                                error => format!("Image selection failed: {error:?}"),
                            };
                            this.finish_history_mutation(false, cx);
                            None
                        }
                    }
                })
                .ok()
                .flatten();
            let Some(path) = path else {
                return;
            };
            let result = cx
                .background_spawn(async move {
                    app_core::relink_history_entry(&database, &entry, &path)
                })
                .await;
            this.update(cx, |this, cx| {
                let refresh = result.is_ok();
                if this.history_visible && this.history_revision == revision {
                    this.history_action_status = match result {
                        Ok(()) => "Image relinked. Image files unchanged.".into(),
                        Err(error) => format!("Could not relink image: {error:#}"),
                    };
                }
                this.finish_history_mutation(refresh, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish_history_export(&mut self, status: String, revision: u64, cx: &mut Context<Self>) {
        self.saving = false;
        self.history_export_cancel = None;
        self.save_status = status.clone();
        if self.history_visible && self.history_revision == revision {
            self.history_action_status = status;
        }
        self.finish_quit_if_idle(cx);
        cx.notify();
    }

    fn cancel_history_export(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.history_export_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            self.save_status = "Stopping export… Completed copies are kept.".into();
            cx.notify();
        }
    }

    fn begin_history_export(
        &mut self,
        sources: Vec<PathBuf>,
        directory: PathBuf,
        revision: u64,
        cx: &mut Context<Self>,
    ) {
        debug_assert!(self.saving && self.history_export_cancel.is_none());
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.history_export_cancel = Some(cancel.clone());
        self.save_status = format!("Exporting {} selected images…", sources.len());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    app_core::export_saved_pngs(&sources, &directory, || {
                        cancel.load(std::sync::atomic::Ordering::Relaxed)
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                let status = match result {
                    Ok(report) => {
                        let mut status = format!(
                            "Exported {} · Failed {} · Not exported {}. Originals kept.",
                            report.exported,
                            report.failures.len(),
                            report.not_exported
                        );
                        for (path, error) in report.failures.iter().take(3) {
                            status.push_str(&format!(
                                "\n{}: {error}",
                                path.file_name().unwrap_or_default().to_string_lossy()
                            ));
                        }
                        if report.failures.len() > 3 {
                            status.push_str("\nOnly the first three errors are shown.");
                        }
                        status
                    }
                    Err(error) => format!("Could not export selected images: {error:#}"),
                };
                this.finish_history_export(status, revision, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn export_selected_history(&mut self, revision: u64, cx: &mut Context<Self>) {
        if self.quitting
            || !self.history_visible
            || self.history_revision != revision
            || self.history_loading
            || self.history_mutating
            || self.capturing
            || self.copying
            || self.saving
            || self.settings_saving
            || self.capture_delay.is_some()
            || self.history_selected.is_empty()
            || self.history_selected.len() > 20
        {
            return;
        }
        let sources: Vec<_> = self
            .history_entries
            .iter()
            .filter(|entry| self.history_selected.contains(&entry.id))
            .map(|entry| entry.path.clone())
            .collect();
        if sources.len() != self.history_selected.len() {
            return;
        }
        self.saving = true;
        let dialog = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Export selected PNGs to a folder (existing files are kept)".into()),
        });
        cx.spawn(async move |this, cx| {
            let selection = dialog.await;
            this.update(cx, |this, cx| {
                if this.quitting || !this.history_visible || this.history_revision != revision {
                    this.finish_history_export(
                        "Export canceled before copying. No files exported.".into(),
                        revision,
                        cx,
                    );
                    return;
                }
                match selection {
                    Ok(Ok(Some(mut paths))) if paths.len() == 1 => {
                        this.begin_history_export(sources, paths.remove(0), revision, cx)
                    }
                    other => {
                        let status = match other {
                            Ok(Ok(None)) => "Export canceled. No files exported.".into(),
                            Ok(Ok(Some(_))) => {
                                "Choose one export folder. No files exported.".into()
                            }
                            error => format!("Export folder selection failed: {error:?}"),
                        };
                        this.finish_history_export(status, revision, cx);
                    }
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn restore_upload_destination(&mut self, cx: &mut Context<Self>) {
        if self.upload_profile_checked || self.upload_config_loading || self.quitting {
            return;
        }
        self.upload_profile_checked = true;
        self.upload_config_loading = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async {
                    app_core::UploadDestination::load_remembered()
                        .map_err(|error| format!("{error:#}"))
                })
                .await;
            this.update(cx, |this, cx| {
                this.upload_config_loading = false;
                if !this.quitting {
                    match result {
                        Ok(Some(destination)) => {
                            this.upload_status = format!(
                                "Destination: {} · Remembered · Manual upload",
                                destination.origin()
                            );
                            this.upload_destination = Some(destination);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            this.upload_status =
                                format!("Uploads disabled · Could not restore destination: {error}")
                        }
                    }
                }
                this.finish_quit_if_idle(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn persist_upload_destination(&mut self, forget: bool, cx: &mut Context<Self>) {
        if self.quitting || self.upload_config_loading || self.upload_cancel.is_some() {
            return;
        }
        let destination = self.upload_destination.clone();
        if !forget && destination.is_none() {
            return;
        }
        self.upload_profile_checked = true;
        self.upload_config_loading = true;
        if forget {
            self.upload_destination = None;
            self.upload_url = None;
        }
        cx.spawn(async move |this,cx| {
            let result = cx.background_spawn(async move {
                if forget { app_core::UploadDestination::forget_remembered() } else { destination.unwrap().remember() }.map_err(|error| format!("{error:#}"))
            }).await;
            this.update(cx, |this,cx| {
                this.upload_config_loading = false;
                this.upload_status = match (forget,result) {
                    (true,Ok(())) => "Uploads disabled · Remembered destination removed".into(),
                    (true,Err(error)) => format!("Uploads disabled for this session · Could not remove remembered destination: {error}"),
                    (false,Ok(())) => format!("Destination: {} · Remembered · Manual upload", this.upload_destination.as_ref().map(|value|value.origin()).unwrap_or_default()),
                    (false,Err(error)) => format!("Destination remains session only · Could not remember it: {error}"),
                };
                this.finish_quit_if_idle(cx); cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }

    fn load_upload_destination(&mut self, cx: &mut Context<Self>) {
        if self.quitting || self.upload_config_loading || self.upload_cancel.is_some() {
            return;
        }
        self.upload_config_loading = true;
        self.upload_profile_checked = true;
        self.upload_destination = None;
        self.upload_url = None;
        self.upload_status = "Choose an upload destination configuration (JSON)".into();
        let dialog = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Load HTTP upload destination (JSON, no credentials)".into()),
        });
        cx.spawn(async move |this, cx| {
            let selected = dialog.await;
            let path = this
                .update(cx, |this, cx| {
                    if this.quitting {
                        this.upload_config_loading = false;
                        this.finish_quit_if_idle(cx);
                        return None;
                    }
                    match selected {
                        Ok(Ok(Some(mut paths))) if paths.len() == 1 => Some(paths.remove(0)),
                        other => {
                            this.upload_config_loading = false;
                            this.upload_status = match other {
                                Ok(Ok(None)) => {
                                    "Uploads disabled · Destination selection canceled".into()
                                }
                                error => format!(
                                    "Uploads disabled · Destination selection failed: {error:?}"
                                ),
                            };
                            this.finish_quit_if_idle(cx);
                            cx.notify();
                            None
                        }
                    }
                })
                .ok()
                .flatten();
            let Some(path) = path else {
                return;
            };
            let result = cx
                .background_spawn(async move {
                    app_core::UploadDestination::load(&path).map_err(|error| format!("{error:#}"))
                })
                .await;
            this.update(cx, |this, cx| {
                this.upload_config_loading = false;
                if !this.quitting {
                    match result {
                        Ok(destination) => {
                            this.upload_status = format!(
                                "Destination: {} · Session only · Manual upload",
                                destination.origin()
                            );
                            this.upload_destination = Some(destination);
                        }
                        Err(error) => this.upload_status = format!("Uploads disabled: {error}"),
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

    fn hide_upload_history(&mut self) {
        self.upload_history_visible = false;
        self.upload_history_revision = self.upload_history_revision.wrapping_add(1);
        if let Some(cancel) = &self.upload_history_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.upload_history_entries.clear();
        self.upload_history_more = false;
        self.upload_history_status.clear();
    }

    fn refresh_upload_history(&mut self, before: Option<i64>, cx: &mut Context<Self>) {
        if self.quitting || !self.upload_history_visible || self.upload_history_loading {
            return;
        }
        let Some(database) = self.history_database.clone() else {
            self.upload_history_status = "Upload history storage unavailable".into();
            return;
        };
        self.upload_history_loading = true;
        self.upload_history_revision = self.upload_history_revision.wrapping_add(1);
        let revision = self.upload_history_revision;
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.upload_history_cancel = Some(cancel.clone());
        self.upload_history_status = "Loading upload history…".into();
        cx.spawn(async move |this,cx| {
            let result = cx.background_spawn(async move { app_core::upload_history_page(&database,before,|| cancel.load(std::sync::atomic::Ordering::Relaxed)).map_err(|error|format!("{error:#}")) }).await;
            this.update(cx, |this,cx| {
                this.upload_history_loading = false; this.upload_history_cancel = None;
                if this.quitting { return; }
                if this.upload_history_visible && revision == this.upload_history_revision {
                    match result {
                        Ok(mut entries) => {
                            this.upload_history_more = entries.len()>20; entries.truncate(20);
                            this.upload_history_status = if entries.is_empty() { "No successful uploads yet" } else { "Successful uploads · Newest first · Removing entries does not delete remote files" }.into();
                            this.upload_history_entries = entries;
                        }
                        Err(error) => this.upload_history_status = format!("Could not load upload history: {error}"),
                    }
                } else if this.upload_history_visible { this.refresh_upload_history(None,cx); }
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }

    fn remove_upload_entry(&mut self, id: i64, revision: u64, cx: &mut Context<Self>) {
        if self.quitting
            || self.history_mutating
            || !self.upload_history_visible
            || self.upload_history_loading
            || revision != self.upload_history_revision
            || !self
                .upload_history_entries
                .iter()
                .any(|entry| entry.id == id)
        {
            return;
        }
        let Some(database) = self.history_database.clone() else {
            return;
        };
        self.upload_history_loading = true;
        self.history_mutating = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    app_core::remove_upload_history_entry(&database, id)
                        .map_err(|error| format!("{error:#}"))
                })
                .await;
            this.update(cx, |this, cx| {
                this.upload_history_loading = false;
                if this.upload_history_visible {
                    if revision != this.upload_history_revision {
                        this.refresh_upload_history(None, cx);
                    } else {
                        match result {
                            Ok(()) => this.refresh_upload_history(None, cx),
                            Err(error) => {
                                this.upload_history_status =
                                    format!("Could not remove entry: {error}")
                            }
                        }
                    }
                }
                this.finish_history_mutation(false, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn start_upload(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.last_saved.clone() else {
            return;
        };
        self.start_upload_source(path, cx);
    }

    fn upload_history_image(&mut self, id: i64, revision: u64, cx: &mut Context<Self>) {
        if !self.history_visible || self.history_loading || revision != self.history_revision {
            return;
        }
        let Some(path) = self
            .history_entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.path.clone())
        else {
            return;
        };
        self.start_upload_source(path, cx);
    }

    fn start_upload_source(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.quitting
            || self.upload_cancel.is_some()
            || self.upload_config_loading
            || self.saving
            || self.capturing
            || self.copying
            || self.history_mutating
            || self.editor.is_some()
            || self.editor_closing
            || self.capture_delay.is_some()
        {
            return;
        }
        let Some(destination) = self.upload_destination.clone() else {
            return;
        };
        let cancel = app_core::UploadCancellation::default();
        self.upload_cancel = Some(cancel.clone());
        self.upload_generation = self.upload_generation.wrapping_add(1);
        self.upload_cancel_requested = false;
        let generation = self.upload_generation;
        let (reporter, mut progress) = app_core::upload_progress_channel();
        self.upload_url = None;
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        self.upload_status = format!(
            "Uploading {filename} via {} to {}…",
            destination.request_method(),
            destination.origin()
        );
        let database = self.history_database.clone();
        let progress_filename = filename.clone();
        cx.spawn(async move |this, cx| {
            while let Some(progress) = progress.next().await {
                if this
                    .update(cx, |this, cx| {
                        if this.quitting
                            || this.upload_generation != generation
                            || this.upload_cancel.is_none()
                            || this.upload_cancel_requested
                        {
                            return false;
                        }
                        this.upload_status = if progress.prepared == progress.total {
                            format!(
                                "{progress_filename} · PNG data prepared · Awaiting server response"
                            )
                        } else {
                            format!(
                                "{progress_filename} · Upload request: {} / {} PNG bytes",
                                progress.prepared, progress.total
                            )
                        };
                        cx.notify();
                        true
                    })
                    .ok()
                    != Some(true)
                {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    app_core::upload_png_with_progress(&destination, &path, cancel, reporter)
                        .map(|url| {
                            let history_error = database
                                .as_ref()
                                .map(|database| {
                                    app_core::record_uploaded_png(
                                        database,
                                        &path,
                                        &destination.origin(),
                                        &url,
                                    )
                                })
                                .transpose()
                                .err()
                                .map(|error| format!("{error:#}"));
                            (url, history_error)
                        })
                        .map_err(|error| format!("{error:#}"))
                })
                .await;
            this.update(cx, |this, cx| {
                this.upload_cancel = None;
                if !this.quitting {
                    match result {
                        Ok((url, history_error)) => {
                            this.upload_status =
                                format!("Uploaded {filename} · Link ready · Clipboard unchanged");
                            this.upload_url = Some(url);
                            if let Some(error) = history_error {
                                this.upload_status
                                    .push_str(&format!(" · History not saved: {error}"));
                            }
                            if this.upload_history_loading {
                                this.upload_history_revision =
                                    this.upload_history_revision.wrapping_add(1);
                                if let Some(cancel) = &this.upload_history_cancel {
                                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                                }
                            } else {
                                this.refresh_upload_history(None, cx);
                            }
                        }
                        Err(error) => this.upload_status = format!("No upload link: {error}"),
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

    fn cancel_upload(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.upload_cancel {
            self.upload_cancel_requested = true;
            cancel.cancel();
            self.upload_status =
                "Canceling upload… The server may already have received the file.".into();
            cx.notify();
        }
    }

    fn save_capture_as(&mut self, cx: &mut Context<Self>) {
        let jpeg_quality = self.jpeg_quality;
        let export_format = self.export_format;
        if self.capture_delay.is_some() {
            return;
        }
        if self.capturing || self.copying || self.saving || self.settings_saving || self.quitting
            || self.editor.is_some() || self.editor_closing || self.preview_active || self.switching_graphics {
            return;
        }
        let size = match self.export_size {
            app_core::ExportSize::Exact(_, _) => {
                let width = self.export_dimension_input.as_ref().and_then(|input| input.read(cx).value().parse::<u32>().ok());
                let height = self.export_height_input.as_ref().and_then(|input| input.read(cx).value().parse::<u32>().ok());
                let (Some(width), Some(height)) = (width.filter(|value| *value > 0), height.filter(|value| *value > 0)) else {
                    self.save_status = "Image not saved: enter positive whole-pixel export width and height.".into();
                    cx.notify(); return;
                };
                app_core::ExportSize::Exact(width, height)
            }
            app_core::ExportSize::Width(_) | app_core::ExportSize::Height(_) => {
                let height = matches!(self.export_size, app_core::ExportSize::Height(_));
                let value = self.export_dimension_input.as_ref().and_then(|input| input.read(cx).value().parse::<u32>().ok());
                let Some(value) = value.filter(|value| *value > 0) else {
                    self.save_status = format!("Image not saved: enter a positive whole-pixel export {}.", if height { "height" } else { "width" });
                    cx.notify(); return;
                };
                if height { app_core::ExportSize::Height(value) } else { app_core::ExportSize::Width(value) }
            }
            size => size,
        };
        if let Err(error) = size.validate() { self.save_status = format!("Image not saved: {error}"); cx.notify(); return; }
        self.export_size = size;
        let shot = self.last_capture.clone();
        if let Some(shot) = &shot {
            if let Err(error) = size.dimensions(shot.pixels.width(), shot.pixels.height()) {
                self.save_status = format!("Image not saved: {error}");
                cx.notify(); return;
            }
        }
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
        let suggested_name = format!("Screenshot.{}", export_format.extension());
        let dialog = cx.prompt_for_new_path(&directory, Some(&suggested_name));
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
            if path.extension().is_none_or(|extension| extension.is_empty()) { path.set_extension(export_format.extension()); }
            let result = cx.background_spawn(async move {
                let result = if let Some(shot) = shot { shot.save_image_as(&path, jpeg_quality, size) } else { app_core::export_saved_png_as(&source, &path, jpeg_quality, size) };
                result.map(|format| {
                    let history_error = if format == app_core::ImageExportFormat::Png && size == app_core::ExportSize::Original {
                        history_database.as_ref().and_then(|database| app_core::record_saved_capture(database, &path).err())
                            .map(|error| format!("{error:#}"))
                    } else { None };
                    (path, format, history_error)
                }).map_err(|error| match error {
                    app_core::CaptureError::Io(error) if error.kind() == std::io::ErrorKind::AlreadyExists =>
                        "The file already exists. Choose a new name; existing files are never overwritten.".to_owned(),
                    error => error.to_string(),
                })
            }).await;
            this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok((path, format, history_error)) => {
                        this.save_status = format!("{} saved: {}", format.label(), path.display());
                        if format == app_core::ImageExportFormat::Png && size == app_core::ExportSize::Original {
                            this.last_saved = Some(path);
                            this.last_capture = None;
                        } else {
                            this.save_status.push_str(" · Exported a copy; current image and library unchanged");
                            if size != app_core::ExportSize::Original { this.save_status.push_str(&format!(" · Size {}", size.label())); }
                        }
                        if let Some(error) = history_error {
                            this.history_status = format!("PNG saved, but history was not updated: {error}");
                            this.save_status.push_str(" · History update failed");
                        } else if this.history_visible && format == app_core::ImageExportFormat::Png && size == app_core::ExportSize::Original { this.refresh_history(None, cx); }
                    }
                    Err(error) => {
                        this.quitting = false;
                        this.save_status = format!("Image not saved: {error}");
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

    fn finish_quick_capture(&mut self, shots: Vec<CapturedImage>, render: Arc<gpui::RenderImage>, profile: CaptureOutputProfile, cx: &mut Context<Self>) {
        let Some(shot) = shots.last().cloned() else { return; };
        if profile.requires_single_image() && shots.len() != 1 {
            self.capture_status = "Choose one display or window for Copy only or Annotate first.".into();
            cx.notify();
            return;
        }
        if profile == CaptureOutputProfile::AnnotateFirst {
            self.show_prepared_editor(shot, render, false, cx);
            if self.editor.is_none() { self.show_control(cx); }
            self.finish_quit_if_idle(cx);
            cx.notify();
            return;
        }
        self.replace_last_render(render, cx);
        self.last_saved = None;
        self.last_capture = Some(shot);
        if !profile.saves() { self.save_status = "Not saved · Image is kept in memory".into(); }
        if !profile.copies() { self.clipboard_status = "Clipboard unchanged (Save only)".into(); }
        self.output_captures(if profile.saves() { shots } else { Vec::new() }, profile.copies(), cx);
    }

    fn output_captures(&mut self, saves: Vec<CapturedImage>, copy: bool, cx: &mut Context<Self>) {
        let filename_template = if saves.is_empty() { String::new() } else { self.filename_template.clone() };
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
                            .map(|mut shot| {
                                let saved = app_core::capture_file_name(&filename_template).and_then(|name| {
                                    if let Some(name) = name { shot.png_path.set_file_name(name); }
                                    shot.save_png().map_err(anyhow::Error::from)
                                }).map_err(|error| error.to_string());
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
                        this.last_saved = results.last().map(|(path, _, _)| path.clone());
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
        profile: CaptureOutputProfile,
        cx: &mut Context<Self>,
    ) {
        if self.capturing || self.copying || self.saving || self.quitting {
            return;
        }
        let edit_selection = self.edit_selection || profile == CaptureOutputProfile::AnnotateFirst;
        let remembered = app_core::RememberedRegion::from_snapshot(&source, region).ok();
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
                        this.set_last_region(remembered);
                        if edit_selection {
                            this.show_prepared_editor(shot, render, true, cx);
                            if this.editor.is_none() { this.show_control(cx); }
                            this.finish_quit_if_idle(cx);
                            cx.notify();
                            return;
                        }
                        tracing::info!(width = shot.width, height = shot.height, path = %shot.png_path.display(), "selection prepared");
                        this.capture_status = format!("Selection {}×{}  {}", shot.width, shot.height, shot.png_path.display());
                        this.finish_quick_capture(vec![shot], render, profile, cx);
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
        let profile = self.output_profile;
        let repeating_region = jobs.iter().any(|job| matches!(job, CaptureJob::RepeatRegion(_)));
        let exact_region = jobs.iter().find_map(|job| match job {
            CaptureJob::ExactRegion(region) => Some(region.clone()), _ => None,
        });
        if !preview && profile.requires_single_image() && jobs.len() != 1 {
            self.capture_status = "Choose one display or window for Copy only or Annotate first.".into();
            cx.notify();
            return;
        }
        if !preview && profile == CaptureOutputProfile::AnnotateFirst && !self.graphics {
            self.attach_ui(UiRequest::Capture(jobs), cx);
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
                            if preview || profile == CaptureOutputProfile::AnnotateFirst {
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
                        if let Some(region) = exact_region { this.set_last_region(Some(region)); }
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
                                        overlay.output_profile = profile;
                                        cx.notify();
                                    })
                                    .ok();
                            }
                        } else {
                            if let Some(render) = renders.get(&shot.png_path).cloned() {
                                this.finish_quick_capture(shots, render, profile, cx);
                            }
                        }
                    }
                    Err(error) => {
                        if repeating_region { this.show_page(ControlPage::Captures, cx); }
                        if repeating_region && matches!(&error, app_core::CaptureError::StaleRegion | app_core::CaptureError::DisplayNotFound(_)) {
                            this.set_last_region(None);
                        }
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

impl Focusable for CaptureWorkspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
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

struct Overlay {
    edit_selection: bool,
    output_profile: CaptureOutputProfile,
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
    owner: gpui::WeakEntity<CaptureWorkspace>,
}

impl Overlay {
    fn new(
        display_id: DisplayId,
        bounds: Bounds<Pixels>,
        mode: OverlayMode,
        snapshot: Option<CapturedImage>,
        render_image: Option<Arc<gpui::RenderImage>>,
        owner: gpui::WeakEntity<CaptureWorkspace>,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            active: true,
            edit_selection: false,
            output_profile: CaptureOutputProfile::default(),
            #[cfg(target_os = "windows")]
            native: native_window(_window)
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
        let profile = self.output_profile;
        cx.defer(move |cx| {
            owner
                .update(cx, |this, cx| this.save_selection(snapshot, region, profile, cx))
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
        let theme = crate::theme::current(cx);
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
                    .border_2().border_color(rgb(theme.accent)).bg(rgba(theme.selection_fill))
            }))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .m_4()
                    .p_4()
                    .rounded(px(theme.panel_radius))
                    .bg(rgb(theme.surface))
                    .text_color(rgb(theme.text))
                    .child(if self.mode == OverlayMode::Probe { format!("{label}  display {id}") } else { "Select a region".into() })
                    .when(self.mode == OverlayMode::Probe, |panel| panel.child(div().text_sm().child(format!(
                        "display origin=({}, {})  size={}×{}  scale={scale:.2}",
                        self.bounds.origin.x,
                        self.bounds.origin.y,
                        self.bounds.size.width,
                        self.bounds.size.height
                    ))))
                    .child(
                        div().text_sm().text_color(rgb(theme.muted)).child(
                            if self.edit_selection || self.output_profile == CaptureOutputProfile::AnnotateFirst { "Drag to capture & annotate. Esc to cancel." } else if self.output_profile == CaptureOutputProfile::CopyOnly { "Drag to copy only. Esc to cancel. No file is saved." } else if self.output_profile == CaptureOutputProfile::SaveOnly { "Drag to save only. Esc to cancel. Clipboard stays unchanged." } else { "Drag to copy & save. Esc to cancel. One display at a time." },
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
            this.clear_history_preview(cx);
            this.cancel_history_export(cx);
            this.cancel_upload(cx);
            this.close_editor(cx);
            this.quitting = true;
            let busy = this.capturing
                || this.copying
                || this.saving
                || this.switching_graphics
                || this.settings_saving
                || this.history_mutating
                || this.upload_config_loading
                || this.upload_cancel.is_some()
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
        if let Some(control) = WindowHandle::<CaptureWorkspace>::from_handle(handle, cx) {
            let waiting = control
                .update(cx, |this, _, cx| {
                    this.cancel_capture_delay(cx);
                    this.clear_history_preview(cx);
                    this.cancel_history_export(cx);
                    this.cancel_upload(cx);
                    this.quitting = true;
                    let busy = this.capturing
                        || this.copying
                        || this.saving
                        || this.settings_saving
                        || this.history_mutating
                        || this.upload_config_loading
                        || this.upload_cancel.is_some()
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
        gpui::Application::with_platform(platform).with_assets(assets::AppAssets).run(move |cx: &mut App| {
            cx.set_global(fonts);
            redaction_editor::bind_keys(cx);
            cx.on_action(|_: &Quit, cx| cx.defer(close_application));
            cx.bind_keys([
                KeyBinding::new("escape", CloseOverlay, None),
                KeyBinding::new("ctrl-shift-o", OpenProbeOverlays, Some("CaptureWorkspace")),
                KeyBinding::new("ctrl-shift-f", OpenFullscreenOverlays, Some("CaptureWorkspace")),
                KeyBinding::new("ctrl-shift-c", CapturePrimary, Some("CaptureWorkspace")),
                KeyBinding::new(if cfg!(target_os = "macos") { "cmd-q" } else { "ctrl-q" }, Quit, None),
            ]);

            let bounds = Bounds::centered(None, size(px(1060.0), px(760.0)), cx);
            cx.open_kit_window(
                WindowOptions {
                    window_min_size: Some(size(px(680.), px(480.))),
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("screenshots".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    let shell = cx.new(|cx| CaptureWorkspace::new(window, cx));
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
    gpui::Application::with_platform(platform).with_assets(assets::AppAssets)
        .with_windowing(gpui::WindowingRequest::Headless)
        .run(move |cx| {
            cx.set_global(fonts);
            cx.set_quit_mode(gpui::QuitMode::Explicit);
            redaction_editor::bind_keys(cx);
            let controller = cx.new(|cx| CaptureWorkspace::new_controller(None, cx));
            cx.set_global(ResidentController(controller.clone()));
            if let Err(error) = listener.lock().unwrap().listen(app_core::request_reopen) {
                tracing::error!(%error, "Could not start launch listener");
                cx.quit();
                return;
            }
            cx.on_action(|_: &Quit, cx| cx.defer(close_application));
            cx.bind_keys([
                KeyBinding::new("escape", CloseOverlay, None),
                KeyBinding::new("ctrl-shift-o", OpenProbeOverlays, Some("CaptureWorkspace")),
                KeyBinding::new("ctrl-shift-f", OpenFullscreenOverlays, Some("CaptureWorkspace")),
                KeyBinding::new("ctrl-shift-c", CapturePrimary, Some("CaptureWorkspace")),
                KeyBinding::new(if cfg!(target_os = "macos") { "cmd-q" } else { "ctrl-q" }, Quit, None),
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
    let application = gpui_platform::application().with_assets(assets::AppAssets);
    let application = if mode == "headless" {
        application.with_windowing(gpui::WindowingRequest::Headless)
    } else {
        application
    };
    let show_window = mode == "empty";
    application.run(move |cx| {
        if show_window {
            cx.open_kit_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1060.0), px(760.0)),
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
    #[ignore = "uses synthetic images, writes PNG fixtures, replaces clipboard and opens editors to verify capture profiles"]
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
        gpui_platform::application().with_assets(assets::AppAssets).with_windowing(gpui::WindowingRequest::Headless).run(move |cx| {
            cx.set_quit_mode(gpui::QuitMode::Explicit);
            let controller = cx.new(|cx| CaptureWorkspace::new_controller(None, cx));
            cx.set_global(ResidentController(controller.clone()));
            controller.update(cx, |this, _| this.history_database = Some(directory.join("history.sqlite3")));
            let owner = controller.downgrade();
            cx.spawn(async move |cx| {
                let checked = async {
                    for profile in [CaptureOutputProfile::SaveOnly, CaptureOutputProfile::CopyOnly, CaptureOutputProfile::SaveAndCopy] {
                        owner.update(cx, |this, cx| {
                            let shot = CapturedImage { display_id: "fixture".into(), window_id: None, width: 2, height: 2,
                                pixels: Arc::new(image::RgbaImage::from_pixel(2, 2, image::Rgba([91, 42, 17, 255]))),
                                png_path: directory.join(match profile { CaptureOutputProfile::CopyOnly => "copy-only.png", CaptureOutputProfile::SaveOnly => "save-only.png", _ => "save-and-copy.png" }),
                                elapsed_ms: 0, source_format: app_core::PixelFormat::Rgba8UnormSrgb, display_geometry: None };
                            this.last_saved = Some(directory.join("old.png"));
                            // The job's captured profile must win over later settings values.
                            this.output_profile = CaptureOutputProfile::AnnotateFirst;
                            let render = render_thumbnail(&shot);
                            this.finish_quick_capture(vec![shot], render, profile, cx);
                        }).map_err(|e| e.to_string())?;
                        let deadline = std::time::Instant::now();
                        loop {
                            cx.background_executor().timer(std::time::Duration::from_millis(20)).await;
                            if owner.update(cx, |this, _| !this.copying && !this.saving).map_err(|e| e.to_string())? { break; }
                            if deadline.elapsed().as_secs() > 5 { return Err("output timed out".to_string()); }
                        }
                        let state = owner.update(cx, |this, _| (this.last_saved.is_some(), this.last_capture.is_some())).map_err(|e| e.to_string())?;
                        if state != (profile.saves(), !profile.saves()) { return Err("incorrect output retention state".into()); }
                        if profile == CaptureOutputProfile::SaveOnly && unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() } != before { return Err("Save only changed clipboard".into()); }
                    }
                    owner.update(cx, |this, cx| this.show_control(cx)).map_err(|e| e.to_string())?;
                    let started = std::time::Instant::now();
                    loop {
                        cx.background_executor().timer(std::time::Duration::from_millis(20)).await;
                        if owner.update(cx, |this, _| this.control.is_some() && !this.switching_graphics).map_err(|e| e.to_string())? { break; }
                        if started.elapsed().as_secs() > 8 { return Err("control window timed out".into()); }
                    }
                    let before_edit = unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() };
                    for region in [false, true] {
                        owner.update(cx, |this, cx| {
                            let shot = CapturedImage { display_id: u64::from(cx.primary_display().unwrap().id()).to_string(), window_id: None, width: 2, height: 2,
                                pixels: Arc::new(image::RgbaImage::from_pixel(2, 2, image::Rgba([91, 42, 17, 255]))),
                                png_path: directory.join("annotate-first.png"), elapsed_ms: 0, source_format: app_core::PixelFormat::Rgba8UnormSrgb, display_geometry: None };
                            if region {
                                this.save_selection(shot, app_core::PixelRegion { x: 0, y: 0, width: 1, height: 2 }, CaptureOutputProfile::AnnotateFirst, cx);
                            } else {
                                let render = render_frame(&shot);
                                this.finish_quick_capture(vec![shot], render, CaptureOutputProfile::AnnotateFirst, cx);
                            }
                        }).map_err(|e| e.to_string())?;
                        let started = std::time::Instant::now();
                        loop {
                            cx.background_executor().timer(std::time::Duration::from_millis(20)).await;
                            if owner.update(cx, |this, _| this.editor.is_some() && !this.capturing).map_err(|e| e.to_string())? { break; }
                            if started.elapsed().as_secs() > 8 { return Err("annotate-first editor timed out".into()); }
                        }
                        owner.update(cx, |this, cx| {
                            assert!(!this.copying && !this.saving && this.last_capture.is_none());
                            assert_eq!(this.last_saved.as_ref(), Some(&directory.join("save-and-copy.png")));
                            this.close_editor(cx);
                        }).map_err(|e| e.to_string())?;
                        let started = std::time::Instant::now();
                        loop {
                            cx.background_executor().timer(std::time::Duration::from_millis(20)).await;
                            if owner.update(cx, |this, _| this.editor.is_none() && !this.editor_closing).map_err(|e| e.to_string())? { break; }
                            if started.elapsed().as_secs() > 8 { return Err("editor cancel timed out".into()); }
                        }
                        if unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() } != before_edit { return Err("Annotate first changed clipboard".into()); }
                        if directory.join("annotate-first.png").exists() { return Err("Annotate first saved source pixels".into()); }
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
            2
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
    #[ignore = "runs the native headless executor with synthetic history; no windows or clipboard writes"]
    fn history_filters_cancel_stale_requests_without_opening_windows() {
        let errors = track_gui_errors();
        let history_errors = errors.clone();
        let root = std::env::temp_dir().join(format!(
            "screenshots-history-ui-{}-{}",
            std::process::id(),
            probe_timestamp()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let capture = root.join("capture.png");
        let imported = root.join("import.png");
        image::RgbaImage::from_pixel(2, 3, image::Rgba([7, 20, 110, 255]))
            .save(&capture)
            .unwrap();
        image::RgbaImage::new(3, 2).save(&imported).unwrap();
        let capture_bytes = std::fs::read(&capture).unwrap();
        let import_bytes = std::fs::read(&imported).unwrap();
        let export_directory = root.join("exported");
        std::fs::create_dir_all(&export_directory).unwrap();
        let database = root.join("history.sqlite3");
        app_core::record_saved_capture(&database, &capture).unwrap();
        app_core::import_capture_files(&database, &[imported.clone()]).unwrap();
        let clipboard =
            unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        let result = Arc::new(Mutex::new(None));
        let report = result.clone();
        gpui::Application::with_platform(gpui_platform::current_platform(false)).with_assets(assets::AppAssets)
            .with_windowing(gpui::WindowingRequest::Headless)
            .run(move |cx| {
                cx.set_quit_mode(gpui::QuitMode::Explicit);
                let controller = cx.new(|cx| CaptureWorkspace::new_controller(None, cx));
                controller.update(cx, |this, cx| {
                    this.history_database = Some(database);
                    cx.notify();
                });
                cx.spawn(async move |cx| {
                    let checked: Result<(), String> = async {
                        // Native startup enumeration can log access errors for unrelated protected windows.
                        let startup = std::time::Instant::now();
                        while controller.update(cx, |this, _| this.refreshing) {
                            if startup.elapsed().as_secs() > 8 {
                                return Err("Startup enumeration timed out".into());
                            }
                            cx.background_executor().timer(std::time::Duration::from_millis(20)).await;
                        }
                        history_errors.store(0, std::sync::atomic::Ordering::Relaxed);
                        controller.update(cx, |this, cx| {
                            this.history_visible = true;
                            this.refresh_history(None, cx);
                            this.set_history_filter(app_core::HistoryFilter::Captures, cx);
                            this.set_history_filter(app_core::HistoryFilter::Imports, cx);
                            this.set_history_query("capture".into(), cx);
                            this.set_history_query("import".into(), cx);
                            assert!(workspace::valid_search("Grüße 😀"));
                            assert!(!workspace::valid_search("line\nbreak"));
                            assert!(!workspace::valid_search(&"x".repeat(513)));
                            Ok::<_, String>(())
                        })?;
                        for phase in 0..5 {
                            if phase > 0 {
                                controller.update(cx, |this, cx| {
                                    if phase == 1 {
                                        this.set_history_filter(
                                            app_core::HistoryFilter::Captures,
                                            cx,
                                        );
                                        let cancel = this
                                            .history_cancel
                                            .clone()
                                            .ok_or("Missing active history cancellation")?;
                                        this.hide_history(cx);
                                        this.refresh_history(None, cx);
                                        if !cancel.load(std::sync::atomic::Ordering::Relaxed)
                                            || this.history_refresh_pending
                                            || !this.history_entries.is_empty()
                                            || !this.history_query.is_empty()
                                            || this.history_search_input.is_some()
                                        {
                                            return Err(
                                                "Hide history did not cancel/release its page"
                                                    .to_string(),
                                            );
                                        }
                                    } else if phase == 2 {
                                        this.history_visible = true;
                                        this.refresh_history(None, cx);
                                    } else if phase == 3 {
                                        let revision = this.history_revision;
                                        this.select_history_page(revision, cx);
                                        if this.history_selected.len() != 1 {
                                            return Err(
                                                "Select all did not select the displayed page"
                                                    .into(),
                                            );
                                        }
                                        this.remove_history_captures(
                                            this.history_selected.clone(),
                                            cx,
                                        );
                                        this.refresh_history(None, cx);
                                        if !this.history_mutating || !this.history_refresh_pending {
                                            return Err(
                                                "Bulk removal did not coalesce a pending refresh"
                                                    .into(),
                                            );
                                        }
                                    } else {
                                        this.history_mutating = true;
                                        this.refresh_history(None, cx);
                                        this.finish_history_mutation(false, cx);
                                        if this.history_mutating || this.history_refresh_pending || !this.history_loading {
                                            return Err("Canceled/failed metadata write stranded its pending refresh".into());
                                        }
                                    }
                                    Ok(())
                                })?;
                            }
                            let deadline = std::time::Instant::now();
                            loop {
                                if controller.update(cx, |this, _| {
                                    !this.history_loading
                                        && !this.capturing
                                        && !this.history_mutating
                                        && !this.history_refresh_pending
                                        && !this.refreshing
                                }) {
                                    break;
                                }
                                if deadline.elapsed().as_secs() > 8 {
                                    return Err("History filter test timed out".into());
                                }
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(20))
                                    .await;
                            }
                            controller.update(cx, |this, cx| {
                                let expected = if phase == 0 {
                                    "import.png"
                                } else {
                                    "capture.png"
                                };
                                if phase == 1 {
                                    if this.history_visible
                                        || !this.history_entries.is_empty()
                                        || !this.history_status.is_empty()
                                        || this.history_preview.is_some()
                                        || this.history_preview_cancel.is_some()
                                        || !this.history_preview_status.is_empty()
                                    {
                                        return Err(
                                            "A hidden history request applied stale rows/status"
                                                .into(),
                                        );
                                    }
                                } else if phase >= 3 {
                                    if !this.history_entries.is_empty()
                                        || !this.history_selected.is_empty()
                                        || !this.history_action_status.contains("Removed 1")
                                    {
                                        return Err(
                                            "Bulk removal did not refresh/clear the selected page"
                                                .into(),
                                        );
                                    }
                                } else if this.history_entries.len() != 1
                                    || this.history_entries[0].path.file_name().unwrap() != expected
                                {
                                    return Err("History applied an obsolete filter result".into());
                                }
                                if this.history_cancel.is_some()
                                    || this.graphics
                                    || !cx.windows().is_empty()
                                {
                                    return Err(
                                        "History retained work or opened a graphics window".into(),
                                    );
                                }
                                if phase == 0 {
                                    let revision = this.history_revision;
                                    let id = this.history_entries[0].id;
                                    this.toggle_history_selection(id, revision.wrapping_sub(1), cx);
                                    if !this.history_selected.is_empty() {
                                        return Err(
                                            "Stale selection changed the current page".into()
                                        );
                                    }
                                    this.toggle_history_selection(id, revision, cx);
                                    if this.history_selected != vec![id] {
                                        return Err("Row selection failed".into());
                                    }
                                    this.toggle_history_selection(id, revision, cx);
                                    if !this.history_selected.is_empty() {
                                        return Err("Row deselection failed".into());
                                    }
                                    this.relink_history_image(id, revision.wrapping_sub(1), cx);
                                    this.relink_history_image(i64::MAX, revision, cx);
                                    if this.history_mutating {
                                        return Err("A stale/missing entry opened the relink picker".into());
                                    }
                                    this.history_mutating = true;
                                    this.relink_history_image(id, revision, cx);
                                    this.set_history_filter(app_core::HistoryFilter::Captures, cx);
                                    this.history_mutating = false;
                                    if this.history_filter != app_core::HistoryFilter::Imports
                                        || this.history_revision != revision
                                    {
                                        return Err(
                                            "History changed filters during a metadata write"
                                                .into(),
                                        );
                                    }
                                    this.preview_history_image(id, revision, cx);
                                    if !this.capturing || this.history_preview_cancel.is_none() {
                                        return Err("History preview did not start a bounded image job".into());
                                    }
                                    let active = this.history_preview_cancel.clone().unwrap();
                                    this.preview_history_image(id, revision, cx);
                                    if !Arc::ptr_eq(&active, this.history_preview_cancel.as_ref().unwrap()) {
                                        return Err("Duplicate preview created another job".into());
                                    }
                                }
                                Ok::<_, String>(())
                            })?;
                            if phase == 2 {
                                controller.update(cx, |this, cx| {
                                    this.preview_history_image(this.history_entries[0].id, this.history_revision, cx);
                                });
                                let started = std::time::Instant::now();
                                while controller.update(cx, |this, _| this.capturing) {
                                    if started.elapsed().as_secs() > 8 {
                                        return Err("History preview timed out".into());
                                    }
                                    cx.background_executor().timer(std::time::Duration::from_millis(20)).await;
                                }
                                controller.update(cx, |this, cx| {
                                    let (id, render) = this.history_preview.as_ref().ok_or("History preview was not published")?;
                                    let bytes = render.as_bytes(0).ok_or("History preview has no pixel data")?;
                                    if bytes.len() != 2 * 3 * 4 || bytes[..4] != [110,20,7,255] {
                                        return Err("Preview resized a small source or lost BGRA color order".into());
                                    }
                                    let id = *id;
                                    let released = Arc::downgrade(render);
                                    this.preview_history_image(id, this.history_revision, cx);
                                    if this.history_preview.is_some() || released.upgrade().is_some()
                                        || this.history_preview_cancel.is_some() || !this.history_preview_status.is_empty() {
                                        return Err("Hide preview retained its image/job/status".into());
                                    }
                                    Ok::<_, String>(())
                                })?;
                                controller.update(cx, |this, cx| {
                                    this.saving = true;
                                    this.begin_history_export(vec![this.history_entries[0].path.clone()], export_directory.clone(), this.history_revision, cx);
                                });
                                let started = std::time::Instant::now();
                                while controller.update(cx, |this, _| this.saving) {
                                    if started.elapsed().as_secs() > 8 { return Err("Selected export timed out".into()); }
                                    cx.background_executor().timer(std::time::Duration::from_millis(20)).await;
                                }
                                controller.update(cx, |this, _| {
                                    if this.history_export_cancel.is_some() || !this.save_status.starts_with("Exported 1")
                                        || this.last_saved.is_some() || this.last_capture.is_some() {
                                        return Err("Selected export lost its result or changed the last capture".into());
                                    }
                                    Ok::<_, String>(())
                                })?;
                            }
                        }
                        controller.update(cx, |this, cx| this.hide_history(cx));
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
            .expect("History filter test did not finish")
            .unwrap();
        assert_eq!(std::fs::read(&capture).unwrap(), capture_bytes);
        assert_eq!(std::fs::read(&imported).unwrap(), import_bytes);
        assert_eq!(
            std::fs::read(root.join("exported/capture.png")).unwrap(),
            capture_bytes
        );
        assert_eq!(std::fs::read_dir(root.join("exported")).unwrap().count(), 1);
        let remaining =
            app_core::capture_history_page(&root.join("history.sqlite3"), None).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].path, imported);
        assert_eq!(
            unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() },
            clipboard
        );
        assert_eq!(errors.load(std::sync::atomic::Ordering::Relaxed), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "native headless upload UI with a synthetic PNG and loopback server; no clipboard writes"]
    fn manual_upload_keeps_clipboard_and_capture_unchanged() {
        use std::io::{Read, Write};
        let errors = track_gui_errors();
        let event_errors = errors.clone();
        let root = std::env::temp_dir().join(format!(
            "screenshots-upload-ui-{}-{}",
            std::process::id(),
            probe_timestamp()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("saved.png");
        image::RgbaImage::new(3, 2).save(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        let source = path.clone();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let config = root.join("destination.json");
        std::fs::write(&config, format!(r#"{{"url":"http://{}/upload","field":"file","json_pointer":"/url","allow_http":true}}"#, listener.local_addr().unwrap())).unwrap();
        let destination = app_core::UploadDestination::load(&config).unwrap();
        let server = std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && started.elapsed().as_secs() < 5 =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(1))
                    }
                    error => panic!("Synthetic upload accept failed: {error:?}"),
                }
            };
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            stream
                .set_write_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut headers = Vec::new();
            let mut byte = [0];
            while !headers.ends_with(b"\r\n\r\n") {
                assert!(headers.len() < 8192);
                stream.read_exact(&mut byte).unwrap();
                headers.push(byte[0]);
            }
            let headers = String::from_utf8(headers).unwrap();
            assert!(headers.starts_with("POST /upload HTTP/1.1"));
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            assert!(length < 8192);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            let response = r#"{"url":"https://example.invalid/share/native"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                response.len()
            )
            .unwrap();
        });
        let clipboard =
            unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() };
        let result = Arc::new(Mutex::new(None));
        let report = result.clone();
        gpui::Application::with_platform(gpui_platform::current_platform(false)).with_assets(assets::AppAssets)
            .with_windowing(gpui::WindowingRequest::Headless)
            .run(move |cx| {
                cx.set_quit_mode(gpui::QuitMode::Explicit);
                let controller = cx.new(|cx| CaptureWorkspace::new_controller(None, cx));
                cx.spawn(async move |cx| {
                    let checked: Result<(), String> = async {
                        let started = std::time::Instant::now();
                        while controller.update(cx, |this, _| this.refreshing) {
                            if started.elapsed().as_secs() > 8 {
                                return Err("Upload fixture startup timed out".into());
                            }
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(20))
                                .await;
                        }
                        event_errors.store(0, std::sync::atomic::Ordering::Relaxed);
                        controller.update(cx, |this, cx| {
                            this.start_upload(cx);
                            if this.upload_cancel.is_some() {
                                return Err(
                                    "Upload started without a destination/source".to_owned()
                                );
                            }
                            this.upload_destination = Some(destination);
                            this.start_upload(cx);
                            if this.upload_cancel.is_some() {
                                return Err("Upload started without a saved PNG".to_owned());
                            }
                            this.history_database =
                                Some(source.parent().unwrap().join("history.sqlite3"));
                            this.last_saved = Some(source.with_file_name("other-last.png"));
                            this.history_visible = true;
                            this.history_revision = 1;
                            this.history_entries = vec![app_core::HistoryEntry {
                                id: 7,
                                path: source.clone(),
                                width: 3,
                                height: 2,
                                created_at: String::new(),
                            }];
                            this.upload_history_image(7, 0, cx);
                            this.upload_history_image(999, 1, cx);
                            this.history_loading = true;
                            this.upload_history_image(7, 1, cx);
                            this.history_loading = false;
                            this.history_mutating = true;
                            this.upload_history_image(7, 1, cx);
                            this.history_mutating = false;
                            if this.upload_cancel.is_some() {
                                return Err(
                                    "Stale, missing or busy history entry started an upload".into(),
                                );
                            }
                            this.upload_history_image(7, 1, cx);
                            this.upload_history_image(7, 1, cx);
                            Ok(())
                        })?;
                        let started = std::time::Instant::now();
                        while controller.update(cx, |this, _| this.upload_cancel.is_some()) {
                            if started.elapsed().as_secs() > 8 {
                                controller.update(cx, |this, cx| this.cancel_upload(cx));
                                return Err("Upload UI timed out".into());
                            }
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(20))
                                .await;
                        }
                        controller.update(cx, |this, cx| {
                            if this.upload_url.as_deref()
                                != Some("https://example.invalid/share/native")
                                || !this.upload_status.starts_with("Uploaded saved.png")
                                || this.last_saved.as_ref()
                                    != Some(&source.with_file_name("other-last.png"))
                                || this.last_capture.is_some()
                                || !cx.windows().is_empty()
                                || this.graphics
                            {
                                return Err(format!(
                                    "Upload UI did not preserve state or publish its link: {}",
                                    this.upload_status
                                ));
                            }
                            Ok(())
                        })?;
                        controller.update(cx, |this, cx| {
                            this.upload_history_visible = true;
                            this.refresh_upload_history(None, cx);
                            this.hide_upload_history();
                            this.upload_history_visible = true;
                            this.refresh_upload_history(None, cx);
                        });
                        for phase in 0..2 {
                            let started = std::time::Instant::now();
                            while controller.update(cx, |this, _| this.upload_history_loading) {
                                if started.elapsed().as_secs() > 8 {
                                    return Err("Upload history timed out".into());
                                }
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(20))
                                    .await;
                            }
                            controller.update(cx, |this, cx| {
                                if phase == 0 {
                                    if this.upload_history_entries.len() != 1
                                        || this.upload_history_entries[0].url
                                            != "https://example.invalid/share/native"
                                        || this.upload_history_entries[0].filename != "saved.png"
                                    {
                                        return Err(
                                            "Successful upload was not restored from history"
                                                .to_owned(),
                                        );
                                    }
                                    let id = this.upload_history_entries[0].id;
                                    this.remove_upload_entry(id, this.upload_history_revision, cx);
                                } else {
                                    if !this.upload_history_entries.is_empty()
                                        || this.history_mutating
                                    {
                                        return Err(
                                            "Upload history removal did not finish".to_owned()
                                        );
                                    }
                                    this.hide_upload_history();
                                }
                                Ok(())
                            })?;
                        }
                        Ok(())
                    }
                    .await;
                    *report.lock().unwrap() = Some(checked);
                    cx.update(|cx| cx.quit());
                })
                .detach();
            });
        server.join().unwrap();
        result
            .lock()
            .unwrap()
            .take()
            .expect("Upload UI did not finish")
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(
            app_core::upload_history_page(&root.join("history.sqlite3"), None, || false)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() },
            clipboard
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
            (gpui::Application::with_platform(platform).with_assets(assets::AppAssets), fonts)
        };
        application
            .with_windowing(gpui::WindowingRequest::Headless)
            .run(move |cx| {
                cx.set_global(fonts);
                cx.set_quit_mode(gpui::QuitMode::Explicit);
                let controller = cx.new(|cx| CaptureWorkspace::new_controller(None, cx));
                cx.set_global(ResidentController(controller.clone()));
                controller.update(cx, |this, _| {
                    this.history_database = Some(directory.join("history.sqlite3"));
                    this.output_directory = Some(directory);
                    this.backend = XcapBackend::new(this.output_directory.as_ref().unwrap());
                });
                let style_fonts = cx.global::<caption::CaptionFonts>().0.clone();
                let owner = controller.downgrade();
                cx.spawn(async move |cx| {
                    let checked = async {
                        cx.background_spawn(async move {
                            let mut masks = Vec::new();
                            for (bold, italic) in [(false, false), (true, false), (false, true), (true, true)] {
                                let asset = caption::rasterize(style_fonts.as_ref(), "Style 123".into(), 24, 0x22c55e, bold, italic, || false)?;
                                if asset.bold != bold || asset.italic != italic || asset.stamp.width > 1024 || asset.stamp.height > 128 || asset.bytes() > 4 * 1024 * 1024 {
                                    return Err("Native text style lost metadata or exceeded its admission bounds".to_string());
                                }
                                masks.push(asset.stamp.mask);
                            }
                            if (1..masks.len()).any(|index| masks[..index].contains(&masks[index])) {
                                return Err("Native style variants did not change the shaped glyphs".to_string());
                            }
                            let single = caption::rasterize(style_fonts.as_ref(), "Row".into(), 16, 0x22c55e, false, false, || false)?;
                            for (text, rows) in [("Row\n\nRow", 3), ("Row\n", 2)] {
                                let block = caption::rasterize(style_fonts.as_ref(), text.into(), 16, 0x22c55e, false, false, || false)?;
                                let row_bytes = single.stamp.mask.len();
                                if block.stamp.width != single.stamp.width || block.stamp.height != single.stamp.height * rows
                                    || block.stamp.mask[..row_bytes] != *single.stamp.mask
                                    || block.stamp.mask[row_bytes..2*row_bytes].iter().any(|alpha| *alpha != 0)
                                    || (rows == 3 && block.stamp.mask[2*row_bytes..] != *single.stamp.mask)
                                {
                                    return Err("Native multiline layout did not preserve blank/trailing rows or glyph pixels".to_string());
                                }
                            }
                            for (text, size) in [("A\nB\nC\nD\nE", 16), ("Row\nRow\nRow\nRow", 36)] {
                                if caption::rasterize(style_fonts.as_ref(), text.into(), size, 0, false, false, || false).is_ok() {
                                    return Err("Native multiline rasterization exceeded its line/mask limits".to_string());
                                }
                            }
                            Ok::<_, String>(())
                        }).await?;
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
        gpui_platform::application().with_assets(assets::AppAssets)
            .with_windowing(gpui::WindowingRequest::Headless)
            .run(move |cx| {
                cx.set_quit_mode(gpui::QuitMode::Explicit);
                let controller = cx.new(|cx| CaptureWorkspace::new_controller(None, cx));
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
                        display_geometry: None,
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
        gpui_platform::application().with_assets(assets::AppAssets)
            .with_windowing(gpui::WindowingRequest::Headless)
            .run(move |cx| {
                cx.set_quit_mode(gpui::QuitMode::Explicit);
                let controller = cx.new(|cx| CaptureWorkspace::new_controller(None, cx));
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
        gpui_platform::application().with_assets(assets::AppAssets).with_windowing(gpui::WindowingRequest::Headless).run(move |cx| {
            cx.set_quit_mode(gpui::QuitMode::Explicit);
            let controller = cx.new(|cx| CaptureWorkspace::new_controller(None, cx));
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
                            if !this.hotkey.as_ref().is_some_and(|key| key.is_registered("Ctrl+Shift+R", app_core::CaptureCommand::Region)) {
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
    fn save_ui_probe_window(
        directory: &std::path::Path,
        filename: &str,
        window_id: &str,
    ) -> Result<(), String> {
        std::fs::create_dir_all(directory).map_err(|e| e.to_string())?; // XCap excludes this process; target only our synthetic HWND.
        let script = r#"
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class UiWindowProbe {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out Rect r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
}
'@
$handle = [IntPtr]::new([Int64]$env:SCREENSHOTS_UI_PROBE_HWND)
$rect = New-Object UiWindowProbe+Rect
if (![UiWindowProbe]::GetWindowRect($handle, [ref]$rect)) { throw 'Synthetic window missing' }
$bitmap = New-Object Drawing.Bitmap ($rect.Right-$rect.Left), ($rect.Bottom-$rect.Top)
$graphics = [Drawing.Graphics]::FromImage($bitmap)
try {
    $dc = $graphics.GetHdc()
    try { if (![UiWindowProbe]::PrintWindow($handle, $dc, 2)) { throw 'Synthetic window capture failed' } }
    finally { $graphics.ReleaseHdc($dc) }
    $bitmap.Save($env:SCREENSHOTS_UI_PROBE_PATH, [Drawing.Imaging.ImageFormat]::Png)
} finally { $graphics.Dispose(); $bitmap.Dispose() }
"#;
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("SCREENSHOTS_UI_PROBE_HWND", window_id)
            .env("SCREENSHOTS_UI_PROBE_PATH", directory.join(filename))
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("synthetic window capture failed".to_string());
        }
        Ok::<_, String>(())
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "opens the editor with synthetic pixels and saves only its own window for UI inspection"]
    fn editor_toolbar_visual_probe() {
        let errors = track_gui_errors();
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-preview");
        let pixels = image::RgbaImage::from_fn(1280, 720, |x, y| {
            let color = if y < 72 {
                [226, 232, 242, 255]
            } else if x < 220 {
                [240, 244, 250, 255]
            } else if x > 280 && x < 1180 && y > 145 && y < 240 {
                [208, 226, 253, 255]
            } else if x > 280 && x < 1180 && y > 300 && y % 80 < 32 {
                [228, 235, 244, 255]
            } else {
                [252, 253, 255, 255]
            };
            image::Rgba(color)
        });
        let source = CapturedImage {
            display_id: "synthetic-ui".into(),
            window_id: None,
            width: 1280,
            height: 720,
            png_path: directory.join("unused-source.png"),
            pixels: Arc::new(pixels),
            elapsed_ms: 0,
            source_format: app_core::PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        };
        let result = Arc::new(Mutex::new(None));
        let report = result.clone();
        let platform = gpui_platform::current_platform(false);
        let fonts = caption::CaptionFonts(platform.text_system());
        gpui::Application::with_platform(platform).with_assets(assets::AppAssets)
            .with_windowing(gpui::WindowingRequest::Headless)
            .run(move |cx| {
                cx.set_global(fonts);
                cx.set_quit_mode(gpui::QuitMode::Explicit);
                let controller = cx.new(|cx| CaptureWorkspace::new_controller(None, cx));
                cx.set_global(ResidentController(controller.clone()));
                controller.update(cx, |this, cx| {
                    this.last_capture = Some(source);
                    this.output_directory = Some(directory.clone());
                    this.show_control(cx);
                });
                let owner = controller.downgrade();
                cx.spawn(async move |cx| {
                    let checked = async {
                        let deadline = std::time::Instant::now();
                        loop {
                            if owner
                                .update(cx, |this, _| {
                                    this.control.is_some() && !this.switching_graphics
                                })
                                .map_err(|e| e.to_string())?
                            {
                                break;
                            }
                            if deadline.elapsed().as_secs() > 10 {
                                return Err("control did not open".to_string());
                            }
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(5))
                                .await;
                        }
                        owner
                            .update(cx, |this, cx| this.open_redaction_editor(cx))
                            .map_err(|e| e.to_string())?;
                        let deadline = std::time::Instant::now();
                        loop {
                            if owner
                                .update(cx, |this, _| this.editor.is_some())
                                .map_err(|e| e.to_string())?
                            {
                                break;
                            }
                            if deadline.elapsed().as_secs() > 10 {
                                return Err("editor did not open".to_string());
                            }
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(5))
                                .await;
                        }
                        for (width, height, filename) in [
                            (1000., 760., "editor-wide.png"),
                            (760., 500., "editor-narrow.png"),
                        ] {
                            let window_id = owner
                                .update(cx, |this, cx| {
                                    this.editor
                                        .unwrap()
                                        .update(cx, |editor, window, cx| {
                                            editor.prepare_toolbar_probe(cx);
                                            use raw_window_handle::{
                                                HasWindowHandle, RawWindowHandle,
                                            };
                                            window.resize(size(px(width), px(height)));
                                            show_control_window(window).unwrap();
                                            match HasWindowHandle::window_handle(window)
                                                .unwrap()
                                                .as_raw()
                                            {
                                                RawWindowHandle::Win32(handle) => {
                                                    handle.hwnd.get().to_string()
                                                }
                                                _ => unreachable!(),
                                            }
                                        })
                                        .unwrap()
                                })
                                .map_err(|e| e.to_string())?;
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(200))
                                .await;
                            let directory = directory.clone();
                            cx.background_executor()
                                .spawn(async move {
                                    std::fs::create_dir_all(&directory)
                                        .map_err(|e| e.to_string())?;
                                    save_ui_probe_window(&directory, filename, &window_id)
                                })
                                .await?;
                        }
                        owner
                            .update(cx, |this, cx| {
                                this.close_editor(cx);
                                this.retire_ui(cx);
                            })
                            .map_err(|e| e.to_string())?;
                        let deadline = std::time::Instant::now();
                        loop {
                            if owner
                                .update(cx, |this, cx| {
                                    if !this.editor_closing && this.control.is_some() {
                                        this.retire_ui(cx);
                                    }
                                    !this.editor_closing
                                        && !this.switching_graphics
                                        && !this.graphics
                                })
                                .map_err(|e| e.to_string())?
                            {
                                break;
                            }
                            if deadline.elapsed().as_secs() > 10 {
                                return Err("editor teardown timed out".to_string());
                            }
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(5))
                                .await;
                        }
                        Ok::<_, String>(())
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
            .expect("visual probe completed")
            .unwrap();
        assert_eq!(errors.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "opens the native control pages; no capture, files or clipboard writes"]
    fn control_pages_render_and_cancel_hidden_history() {
        let errors = track_gui_errors();
        let result = Arc::new(Mutex::new(None));
        let report = result.clone();
        gpui_platform::application().with_assets(assets::AppAssets)
            .with_windowing(gpui::WindowingRequest::Headless)
            .run(move |cx| {
                cx.set_quit_mode(gpui::QuitMode::Explicit);
                let controller = cx.new(|cx| CaptureWorkspace::new_controller(None, cx));
                cx.set_global(ResidentController(controller.clone()));
                controller.update(cx, |this, cx| this.show_control(cx));
                let owner = controller.downgrade();
                cx.spawn(async move |cx| {
                    let checked = async {
                        let deadline = std::time::Instant::now();
                        loop {
                            if owner
                                .update(cx, |this, _| {
                                    this.control.is_some() && !this.switching_graphics
                                })
                                .map_err(|error| error.to_string())?
                            {
                                break;
                            }
                            if deadline.elapsed().as_secs() > 10 {
                                return Err("control opening timed out".to_string());
                            }
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(5))
                                .await;
                        }
                        for page in [
                            ControlPage::Captures,
                            ControlPage::Sources,
                            ControlPage::History,
                            ControlPage::Uploads,
                            ControlPage::Settings,
                            ControlPage::Captures,
                        ] {
                            owner
                                .update(cx, |this, cx| {
                                    let capture_cancel =
                                        Arc::new(std::sync::atomic::AtomicBool::new(false));
                                    let upload_cancel =
                                        Arc::new(std::sync::atomic::AtomicBool::new(false));
                                    if this.page != page {
                                        this.history_visible = true;
                                        this.upload_history_visible = true;
                                        this.history_cancel = Some(capture_cancel.clone());
                                        this.upload_history_cancel = Some(upload_cancel.clone());
                                        this.show_page(page, cx);
                                        assert!(
                                            capture_cancel
                                                .load(std::sync::atomic::Ordering::Relaxed)
                                        );
                                        assert!(
                                            upload_cancel
                                                .load(std::sync::atomic::Ordering::Relaxed)
                                        );
                                        assert_eq!(
                                            this.history_visible,
                                            page == ControlPage::History
                                        );
                                        assert!(!this.upload_history_visible);
                                        assert!(
                                            this.history_entries.is_empty()
                                                && this.upload_history_entries.is_empty()
                                        );
                                        assert!(
                                            this.history_preview.is_none()
                                                && this.history_search_input.is_none()
                                        );
                                    }
                                    assert!(this.page == page);
                                })
                                .map_err(|error| error.to_string())?;
                            // Allow each invalidated native page to render, without injecting input.
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(40))
                                .await;
                        }
                        let window_id = owner
                            .update(cx, |this, cx| {
                                this.control
                                    .unwrap()
                                    .update(cx, |_, window, _| {
                                        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                                        window.resize(size(px(1000.), px(720.)));
                                        match HasWindowHandle::window_handle(window)
                                            .unwrap()
                                            .as_raw()
                                        {
                                            RawWindowHandle::Win32(handle) => {
                                                handle.hwnd.get().to_string()
                                            }
                                            _ => unreachable!(),
                                        }
                                    })
                                    .unwrap()
                            })
                            .map_err(|e| e.to_string())?;
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(80))
                            .await;
                        let empty_window_id = window_id.clone();
                        cx.background_executor()
                            .spawn(async move {
                                let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                                    .join("../../target/ui-preview");
                                save_ui_probe_window(&directory, "workspace-empty.png", &empty_window_id)
                            })
                            .await?;
                        for (width, height, filename, library) in [
                            (1000., 720., "workspace-library.png", true),
                            (680., 480., "workspace-library-compact.png", true),
                            (1000., 720., "workspace-capture-menu.png", false),
                        ] {
                            owner.update(cx, |this, cx| {
                                this.show_page(if library { ControlPage::History } else { ControlPage::Captures }, cx);
                                if library {
                                    this.history_entries = (1..=3).map(|id| app_core::HistoryEntry {
                                        id,
                                        path: PathBuf::from(format!("Example screenshot {id}.png")),
                                        width: 1920,
                                        height: 1080,
                                        created_at: "2026-11-09 14:32".into(),
                                    }).collect();
                                    this.history_selected = vec![1];
                                    this.history_status = "3 example images".into();
                                }
                                this.control.unwrap().update(cx, |_, window, _| window.resize(size(px(width), px(height)))).unwrap();
                                cx.notify();
                            }).map_err(|e| e.to_string())?;
                            cx.background_executor().timer(std::time::Duration::from_millis(80)).await;
                            let window_id = window_id.clone();
                            cx.background_executor().spawn(async move {
                                let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-preview");
                                save_ui_probe_window(&directory, filename, &window_id)
                            }).await?;
                        }
                        for (page, filename) in [
                            (ControlPage::Uploads, "workspace-uploads.png"),
                            (ControlPage::Settings, "workspace-settings.png"),
                        ] {
                            owner.update(cx, |this, cx| {
                                this.show_page(page, cx);
                                this.control.unwrap().update(cx, |_, window, _| window.resize(size(px(1000.), px(720.)))).unwrap();
                            }).map_err(|e| e.to_string())?;
                            cx.background_executor().timer(std::time::Duration::from_millis(80)).await;
                            let window_id = window_id.clone();
                            cx.background_executor().spawn(async move {
                                let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-preview");
                                save_ui_probe_window(&directory, filename, &window_id)
                            }).await?;
                        }
                        owner
                            .update(cx, |this, cx| this.retire_ui(cx))
                            .map_err(|error| error.to_string())?;
                        let deadline = std::time::Instant::now();
                        loop {
                            if owner
                                .update(cx, |this, cx| {
                                    !this.switching_graphics
                                        && !this.graphics
                                        && cx.windows().is_empty()
                                })
                                .map_err(|error| error.to_string())?
                            {
                                break;
                            }
                            if deadline.elapsed().as_secs() > 10 {
                                return Err("control closing timed out".to_string());
                            }
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(5))
                                .await;
                        }
                        Ok::<_, String>(())
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
            .expect("native page test completed")
            .unwrap();
        assert_eq!(errors.load(std::sync::atomic::Ordering::Relaxed), 0);
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
        gpui_platform::application().with_assets(assets::AppAssets)
            .with_windowing(gpui::WindowingRequest::Headless)
            .run(move |cx| {
                cx.set_quit_mode(gpui::QuitMode::Explicit);
                let controller = cx.new(|cx| CaptureWorkspace::new_controller(None, cx));
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
        gpui_platform::application().with_assets(assets::AppAssets).with_windowing(gpui::WindowingRequest::Headless).run(move |cx| {
            cx.set_quit_mode(gpui::QuitMode::Explicit);
            let controller = cx.new(|cx| CaptureWorkspace::new_controller(None, cx));
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
        gpui_platform::application().with_assets(assets::AppAssets).run(move |cx| {
            let control = cx
                .open_kit_window(WindowOptions::default(), |window, cx| {
                    cx.new(|cx| CaptureWorkspace::new(window, cx))
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
                        display_geometry: None,
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
            display_geometry: None,
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
        gpui_platform::application().with_assets(assets::AppAssets).run(move |cx| {
            let control = cx
                .open_kit_window(WindowOptions::default(), |window, cx| {
                    cx.new(|cx| CaptureWorkspace::new(window, cx))
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
                        display_geometry: None,
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
        gpui_platform::application().with_assets(assets::AppAssets).run(move |cx| {
            let opened = cx.open_kit_window(WindowOptions::default(), |window, cx| {
                cx.new(|cx| {
                    let mut shell = CaptureWorkspace::new(window, cx);
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
                                            .map(|handle| AnyWindowHandle::from(*handle).window_id())
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
