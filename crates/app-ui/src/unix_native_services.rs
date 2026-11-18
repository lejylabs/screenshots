//! Existing capture commands and settings controls, backed by singleton-owned native services.
use super::*;

impl CaptureWorkspace {
    pub(super) fn start_unix_native_services(
        &mut self,
        owner: UnixLaunchOwner,
        cx: &mut Context<Self>,
    ) {
        if !self.unix_native_allowed || self.quitting || self.unix_services.is_some() {
            return;
        }
        let wayland = native_capture_context(cx).wayland_session;
        #[cfg(target_os = "linux")]
        let display = cx
            .graphical_environment()
            .and_then(|e| e.x11_display)
            .and_then(|d| d.into_string().ok());
        #[cfg(target_os = "macos")]
        let display = None;
        let (services, mut events) =
            match app_core::UnixDesktopServices::start(owner, wayland, display) {
                Ok(services) => services,
                Err(error) => {
                    self.hotkey_status = format!("Global shortcuts unavailable: {error:#}");
                    self.tray_status = format!("Tray unavailable: {error:#}; Close quits");
                    cx.notify();
                    return;
                }
            };
        self.unix_services = Some(services);
        self.hotkey_status = "Waiting for saved shortcut settings".into();
        self.tray_status = "Starting native tray…".into();
        self.initialize_unix_shortcuts(cx);
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this
                    .update(cx, |this, cx| {
                        if this.unix_services.is_none() {
                            return;
                        }
                        match event {
                            app_core::UnixDesktopEvent::TrayStatus { status, .. } => {
                                this.tray_status = status;
                            }
                            app_core::UnixDesktopEvent::ShortcutStatus(status) => {
                                this.hotkey_status = status
                            }
                            event => {
                                let action =
                                    this.unix_services.as_mut().and_then(|s| s.action(event));
                                if let Some(action) = action {
                                    this.unix_native_action(action, cx);
                                }
                            }
                        }
                        let native = this.unix_services.as_ref().map(|services| {
                            (
                                services.tray_available(),
                                services.tray_status(),
                                services.shortcut_status(),
                            )
                        });
                        if let Some((available, tray, status)) = native {
                            this.tray_status = tray;
                            this.hotkey_status = status;
                            if this.observe_unix_tray(available) {
                                // Restore access after loss without letting an in-flight desktop capture include this window.
                                if let Some(cancel) = &this.native_capture_cancel {
                                    cancel.cancel();
                                }
                                this.cancel_capture_delay(cx);
                                this.unix_reopen(cx);
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }
    pub(super) fn initialize_unix_shortcuts(&mut self, cx: &mut Context<Self>) {
        if self.settings_loading
            || self.output_directory.is_none()
            || self.unix_shortcuts_initialized
            || self.quitting
        {
            return;
        }
        let Some(services) = self.unix_services.as_mut() else {
            return;
        };
        self.unix_shortcuts_initialized = true;
        self.unix_shortcuts_busy = true;
        let operation = services.initialize(&self.hotkey_bindings);
        cx.spawn(async move |this, cx| {
            let report = operation.await;
            this.update(cx, |this, cx| {
                this.unix_shortcuts_busy = false;
                this.hotkey_status = report.status;
                if let Err(error) = report.outcome {
                    this.settings_status = format!("Saved shortcuts could not be applied: {error}");
                }
                this.finish_quit_if_idle(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
    fn unix_native_action(&mut self, action: app_core::UnixDesktopAction, cx: &mut Context<Self>) {
        use app_core::{CaptureCommand as C, TrayCommand as T, UnixDesktopAction as A};
        match action {
            A::Tray(T::Show) => self.unix_reopen(cx),
            A::Tray(T::Quit) => cx.defer(close_application),
            _ if self.image_input_busy() || self.capture_delay.is_some() => {}
            A::Capture(C::Primary) | A::Tray(T::Capture) => self.capture_primary(cx),
            A::Capture(C::Region) | A::Tray(T::Region) => {
                self.open_overlays(OverlayMode::Fullscreen, cx)
            }
            A::Capture(C::EditRegion) | A::Tray(T::EditRegion) => self.select_region_to_edit(cx),
            A::Capture(C::RepeatRegion) | A::Tray(T::RepeatRegion) => self.repeat_last_region(cx),
            A::Capture(C::ActiveWindow) => self.capture_active_window(cx),
            A::Capture(C::ActiveDisplay) => self.capture_active_display(cx),
            A::Tray(T::ActiveWindow) => {
                self.start_delayed_capture(DelayedCapture::ActiveWindow, cx)
            }
            A::Tray(T::ActiveDisplay) => {
                self.start_delayed_capture(DelayedCapture::ActiveDisplay, cx)
            }
        }
    }
    pub(super) fn close_unix_control(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quitting
            || !self
                .unix_services
                .as_ref()
                .is_some_and(|s| s.tray_available())
        {
            cx.defer(close_application);
            return;
        }
        if self.editor.is_some() || self.editor_closing {
            self.capture_status = "Finish or cancel editing before closing this window".into();
            cx.notify();
            return;
        }
        // Close can observe readiness before the queued event receiver does.
        self.unix_tray_observed = true;
        self.minimize_unix_control(window, cx);
    }
    pub(super) fn minimize_unix_control(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_capture_delay(cx);
        self.close_overlays(cx);
        self.preview_active = false;
        if let Some(render) = self.last_render.take() {
            if let Err(error) = window.drop_image(render.clone()) {
                self.last_render = Some(render);
                self.capture_status = format!("Could not release control preview: {error}");
                cx.notify();
                return;
            }
        }
        self.hide_history(cx);
        self.hide_upload_history();
        if self.resident {
            self.retire_ui(cx);
        } else {
            // Unscoped/file-only windows have no resident controller owner.
            window.minimize_window();
        }
    }
    pub(super) fn set_unix_quitting(&mut self, quitting: bool) {
        if self.unix_quit_paused == quitting {
            return;
        }
        self.unix_quit_paused = quitting;
        if let Some(services) = self.unix_services.as_mut() {
            services.set_quitting(quitting);
        }
    }
    pub(super) fn unix_reopen(&mut self, cx: &mut Context<Self>) {
        if self.switching_graphics || (!self.quitting && (self.capturing || self.preview_active)) {
            self.unix_show_pending = true;
            return;
        }
        if self.quitting {
            self.capture_status = "Quitting: waiting for active work to finish…".into();
            self.open_control(cx);
            cx.notify();
        } else {
            self.show_control(cx);
        }
    }
    fn unix_quit_work_pending(&self) -> bool {
        self.saving
            || self.editor_closing
            || self.switching_graphics
            || self.capturing
            || self.refreshing
            || self.copying
            || (self.settings_saving && !self.unix_shortcut_preparing)
            || self.history_mutating
            || self.history_thumb_loading.is_some()
            || self.upload_config_loading
            || self.upload_cancel.is_some()
    }
    pub(super) fn stop_unix_native_services(&mut self, cx: &mut Context<Self>) {
        if self.unix_quit_work_pending() || self.unix_services_stopping {
            return;
        }
        let Some(services) = self.unix_services.take() else {
            return;
        };
        self.hotkey_status = "Global shortcuts stopping".into();
        self.tray_status = "Tray stopping".into();
        #[cfg(target_os = "macos")]
        {
            let _ = cx;
            drop(services);
        }
        #[cfg(target_os = "linux")]
        {
            self.unix_services_stopping = true;
            let shutdown = services.shutdown();
            cx.spawn(async move |this, cx| {
                let result = cx.background_spawn(shutdown).await;
                this.update(cx, |this, cx| {
                    this.unix_services_stopping = false;
                    if let Err(error) = result {
                        this.hotkey_status = format!("Native service cleanup failed: {error:#}");
                    }
                    this.finish_quit_if_idle(cx);
                })
                .ok();
            })
            .detach();
        }
    }
    pub(super) fn save_hotkeys(
        &mut self,
        bindings: app_core::CaptureHotkeys,
        cx: &mut Context<Self>,
    ) {
        if self.page != ControlPage::Settings
            || self.settings_loading
            || self.settings_saving
            || self.unix_shortcuts_busy
            || self.quitting
            || self.unix_services.is_none()
        {
            return;
        }
        let Some(directory) = self.output_directory.clone() else {
            return;
        };
        let bindings = match bindings.validated() {
            Ok(value) => value,
            Err(error) => {
                self.settings_status = format!("Shortcuts unchanged: {error:#}");
                cx.notify();
                return;
            }
        };
        let operation = self.unix_services.as_mut().unwrap().prepare(&bindings);
        let appearance = theme::selected(cx);
        let profile = self.output_profile;
        let presets = self.region_presets.clone();
        let upload_task_defaults = self.upload_task_defaults.clone();
        let output_task_profiles = self.output_task_profiles;
        let jpeg_quality = self.jpeg_quality;
        let export_format = self.export_format;
        let filename_template = self.filename_template.clone();
        let capture_delay_seconds = self.capture_delay_seconds;
        let strip_metadata_on_upload = self.strip_metadata_on_upload;
        self.settings_saving = true;
        self.unix_shortcut_preparing = true;
        self.settings_status = "Reserving shortcuts…".into();
        cx.spawn(async move |this, cx| {
            let report = operation.await;
            let prepared = report.outcome.is_ok();
            let admitted = this.update(cx, |this, cx| this.admit_unix_shortcut_preparation(report, cx)).unwrap_or(false);
            if !admitted {
                if prepared {
                    let rollback = this.update(cx, |this, _| this.unix_services.as_mut().map(|s| s.finish(false))).ok().flatten();
                    rollback_unix_shortcuts(this.clone(), cx, rollback).await;
                }
                return;
            }
            let persisted = bindings.clone();
            let result = cx.background_spawn(async move {
                app_core::CaptureSettings::ensure_output_directory(&directory)?;
                app_core::CaptureSettings::for_directory(directory)?.with_theme(appearance).with_hotkeys(persisted)?.with_output_profile(profile).with_region_presets(presets)?.with_jpeg_quality(jpeg_quality)?.with_export_format(export_format).with_filename_template(filename_template)?.with_capture_delay(capture_delay_seconds)?.with_upload_task_defaults(upload_task_defaults)?.with_output_task_profiles(output_task_profiles).with_strip_metadata_on_upload(strip_metadata_on_upload).save()
            }).await;
            let finish = this.update(cx, |this, _| this.unix_services.as_mut().map(|s| s.finish(result.is_ok()))).ok().flatten();
            let Some(finish) = finish else { return; };
            let cleanup = finish.await;
            this.update(cx, |this, cx| {
                this.settings_saving = false;
                if result.is_ok() { this.hotkey_bindings = bindings; }
                this.hotkey_status = cleanup.status;
                this.settings_status = match (result, cleanup.outcome) {
                    (Ok(()), Ok(())) => if cfg!(target_os = "linux") && native_capture_context(cx).wayland_session { "Shortcut preferences saved; portal assignments are listed below".into() } else { "Shortcuts saved and active".into() },
                    (Err(error), Ok(())) => format!("Shortcuts unchanged; settings were not saved: {error:#}"),
                    (save, Err(error)) => format!("Shortcut cleanup failed: {error}. Settings saved: {}. Active status is listed below; restart to retry cleanup.", save.is_ok()),
                };
                this.hotkey_inputs = None;
                this.finish_quit_if_idle(cx);
                cx.notify();
            }).ok();
        }).detach();
        cx.notify();
    }
    pub(super) fn observe_unix_tray(&mut self, available: bool) -> bool {
        let lost = self.unix_tray_observed && !available;
        self.unix_tray_observed = available;
        lost
    }
    pub(super) fn admit_unix_shortcut_preparation(
        &mut self,
        report: app_core::UnixShortcutReport,
        cx: &mut Context<Self>,
    ) -> bool {
        self.unix_shortcut_preparing = false;
        self.hotkey_status = report.status;
        if let Err(error) = report.outcome {
            self.settings_saving = false;
            self.settings_status = format!("Shortcuts unchanged: {error}");
            self.finish_quit_if_idle(cx);
            cx.notify();
            return false;
        }
        // A successful unpersisted reservation remains busy until rollback completes.
        self.settings_status = if self.quitting {
            "Rolling back shortcuts…"
        } else {
            "Saving shortcuts…"
        }
        .into();
        cx.notify();
        !self.quitting
    }
    pub(super) fn capture_active_window(&mut self, cx: &mut Context<Self>) {
        self.capture_unix_focused(false, cx);
    }
    pub(super) fn capture_active_display(&mut self, cx: &mut Context<Self>) {
        self.capture_unix_focused(true, cx);
    }
    fn capture_unix_focused(&mut self, display: bool, cx: &mut Context<Self>) {
        if self.image_input_busy() || self.capture_delay.is_some() || self.preview_active {
            return;
        }
        if native_capture_context(cx).wayland_session {
            self.capture_status = "Active target capture is unavailable in this Wayland session; use the existing portal source selection".into();
            self.show_control(cx);
            cx.notify();
            return;
        }
        let task = if display { 5 } else { 3 };
        self.apply_upload_task_default(task, cx);
        let profile = self.output_profile_for_task(Some(task));
        self.start_capture_work(Vec::new(), false, profile, Some(display), cx);
    }
}

#[cfg(test)]
#[path = "unix_native_services_tests.rs"]
mod tests;

pub(super) async fn rollback_unix_shortcuts(
    this: gpui::WeakEntity<CaptureWorkspace>,
    cx: &mut gpui::AsyncApp,
    operation: Option<app_core::UnixShortcutOperation>,
) {
    let report = match operation {
        Some(operation) => Some(operation.await),
        None => None,
    };
    this.update(cx, |this, cx| {
        this.settings_saving = false;
        if let Some(report) = report {
            this.hotkey_status = report.status;
            this.settings_status = match report.outcome {
                Ok(()) => "Shortcut changes canceled before settings were saved".into(),
                Err(error) => {
                    format!("Shortcut rollback failed: {error}; active status is listed below")
                }
            };
        }
        this.finish_quit_if_idle(cx);
        cx.notify();
    })
    .ok();
}
