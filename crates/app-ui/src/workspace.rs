//! Kit-based workspace presentation; jobs, persistence and pixels belong to the controller.
use super::*;
use gpui::TestSupportExt as _;
use gpui::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    menu::{DropdownMenu as _, PopupMenuItem},
    radio::RadioGroup,
    scroll::ScrollableElement as _,
    sidebar::{Sidebar, SidebarItem as _, SidebarMenu, SidebarMenuItem},
};

pub(super) fn valid_search(value: &str) -> bool {
    value.len() <= 512 && !value.chars().any(char::is_control)
}

pub(super) fn exact_region_request(display: &DisplayInfo, values: &[String; 4]) -> Result<app_core::RememberedRegion, String> {
    let mut numbers = [0; 4];
    for (value, number) in values.iter().zip(&mut numbers) {
        if value.is_empty() || value.len() > 10 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("Enter X, Y, width and height as whole physical pixel values.".into());
        }
        *number = value.parse::<u32>().map_err(|_| "A pixel value is too large.".to_string())?;
    }
    let area = app_core::PixelRegion { x: numbers[0], y: numbers[1], width: numbers[2], height: numbers[3] };
    app_core::RememberedRegion::from_display(display, area)
        .map_err(|error| match error {
            app_core::CaptureError::MemoryLimit => "The display or region exceeds the 256 MiB image limit.".into(),
            app_core::CaptureError::StaleRegion | app_core::CaptureError::EmptyFrame => "Display geometry is unavailable. Refresh displays.".into(),
            _ => "The region must have a positive size and fit inside this display. Refresh displays if their geometry changed.".into(),
        })
}

impl Render for CaptureWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.page == ControlPage::Captures && (self.last_capture.is_some() || self.last_saved.is_some()) {
            if let app_core::ExportSize::Width(value) | app_core::ExportSize::Height(value) | app_core::ExportSize::Exact(value, _) = self.export_size {
                if self.export_dimension_input.is_none() {
                    self.export_dimension_input = Some(cx.new(|cx| gpui::component::input::InputState::new(window, cx)
                        .default_value(value.to_string()).validate(|value, _| value.len() <= 10 && value.bytes().all(|byte| byte.is_ascii_digit()))));
                }
            } else { self.export_dimension_input = None; }
        } else { self.export_dimension_input = None; }
        if self.export_dimension_input.is_some() && let app_core::ExportSize::Exact(_, height) = self.export_size {
            if self.export_height_input.is_none() {
                self.export_height_input = Some(cx.new(|cx| gpui::component::input::InputState::new(window, cx)
                    .default_value(height.to_string()).validate(|value, _| value.len() <= 10 && value.bytes().all(|byte| byte.is_ascii_digit()))));
            }
        } else { self.export_height_input = None; }
        if self.page == ControlPage::Settings && !self.settings_loading && self.jpeg_quality_input.is_none() {
            self.jpeg_quality_input = Some(cx.new(|cx| gpui::component::input::InputState::new(window, cx)
                .default_value(self.jpeg_quality.to_string())
                .validate(|value, _| value.len() <= 3 && value.bytes().all(|byte| byte.is_ascii_digit()))));
        }
        if self.page != ControlPage::Settings { self.jpeg_quality_input = None; }
        if self.page == ControlPage::Settings && !self.settings_loading && self.filename_template_input.is_none() {
            self.filename_template_input = Some(cx.new(|cx| gpui::component::input::InputState::new(window, cx)
                .default_value(self.filename_template.clone()).placeholder("Default names")
                .validate(|value, _| value.len() <= 96 && !value.chars().any(char::is_control))));
        }
        if self.page != ControlPage::Settings { self.filename_template_input = None; }
        if self.page == ControlPage::Sources && self.backend.capabilities().capture_region && self.exact_region_inputs.is_none() {
            self.exact_region_inputs = Some(std::array::from_fn(|index| cx.new(|cx|
                gpui::component::input::InputState::new(window, cx)
                    .default_value(["0", "0", "400", "300"][index])
                    .validate(|value, _| value.len() <= 10 && value.bytes().all(|byte| byte.is_ascii_digit())))));
            self.region_preset_name = Some(cx.new(|cx| gpui::component::input::InputState::new(window, cx)
                .placeholder("Preset name")
                .validate(|value, _| value.len() <= 64 && !value.chars().any(char::is_control))));
        }
        if self.page != ControlPage::Sources { self.exact_region_inputs = None; self.region_preset_name = None; }
        #[cfg(target_os = "windows")]
        if self.page == ControlPage::Settings && !self.settings_loading && self.hotkey_inputs.is_none() {
            self.hotkey_inputs = Some(std::array::from_fn(|index| {
                cx.new(|cx| gpui::component::input::InputState::new(window, cx)
                    .default_value(self.hotkey_bindings.0[index].clone())
                    .placeholder("Disabled")
                    .validate(|value, _| value.len() <= 64 && value.is_ascii() && !value.chars().any(char::is_control)))
            }));
        }
        #[cfg(target_os = "windows")]
        if self.page != ControlPage::Settings { self.hotkey_inputs = None; }
        let theme = crate::theme::current(cx);
        let colors = cx.theme().colors;
        let compact = window.viewport_size().width < px(820.);
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
        let displays = if self.page == ControlPage::Sources {
            self.xcap_displays.clone()
        } else {
            Vec::new()
        };
        let windows: Vec<_> = self
            .xcap_windows
            .iter()
            .filter(|window| {
                self.page == ControlPage::Sources
                    && !window.is_minimized
                    && !window.title.is_empty()
                    && window.pid != std::process::id()
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
        let drop_owner = cx.entity().downgrade();
        div()
            .id("capture-home")
            .font_family(crate::theme::font(cx))
            .track_focus(&self.focus_handle)
            .key_context("CaptureWorkspace")
            .flex()
            .flex_row()
            .size_full()
            .bg(colors.background)
            .text_color(colors.foreground)
            .can_drop(move |value, _, cx| value.is::<gpui::ExternalPaths>()
                && drop_owner.upgrade().is_some_and(|owner| !owner.read(cx).image_input_busy()))
            .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, _, cx| {
                this.open_dropped_image(paths.paths(), cx);
            }))
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
            .on_action(cx.listener(|this, _: &NewScreenshot, _, cx| this.select_region_to_edit(cx)))
            .on_action(cx.listener(|this, _: &OpenImage, _, cx| this.open_image(cx)))
            .on_action(cx.listener(|this, _: &PasteImage, _, cx| this.paste_image(cx)))
            .on_action(cx.listener(|this, _: &ShowRecent, window, cx| { window.focus(&this.focus_handle, cx); this.show_page(ControlPage::Captures, cx); }))
            .on_action(cx.listener(|this, _: &ShowLibrary, window, cx| { window.focus(&this.focus_handle, cx); this.show_page(ControlPage::History, cx); }))
            .on_action(cx.listener(|this, _: &ShowSources, window, cx| { window.focus(&this.focus_handle, cx); this.show_page(ControlPage::Sources, cx); }))
            .on_action(cx.listener(|this, _: &ShowSettings, window, cx| { window.focus(&this.focus_handle, cx); this.show_page(ControlPage::Settings, cx); }))
            .on_action(|_: &Quit, _, cx| cx.defer(close_application))
            .child(self.render_sidebar(unavailable, window, cx))
            .child(div().id("control-content").flex_1().min_w_0().min_h_0().h_full().overflow_y_scrollbar()
                .child(div().flex().flex_col().gap_4().p_4().when(!theme.classic && !compact, |content| content.p_6())
                .when(theme.classic, |content| content.gap_2())
                .child(div().flex_none().flex().items_start().justify_between().gap_3().pb_3()
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_2()
                        .child(div().id("workspace-heading").role(gpui::Role::Heading).test_support().text_2xl().font_weight(gpui::FontWeight::SEMIBOLD)
                            .when(theme.classic, |heading| heading.text_size(px(14.)))
                            .aria_label(match self.page {
                                ControlPage::Captures => "Recent capture", ControlPage::History => "Library",
                                ControlPage::Sources => "Displays & windows", ControlPage::Uploads => "Uploads",
                                ControlPage::Settings => "Settings",
                            })
                            .child(match self.page {
                            ControlPage::Captures => "Recent capture",
                            ControlPage::History => "Library",
                            ControlPage::Sources => "Displays & windows",
                            ControlPage::Uploads => "Uploads",
                            ControlPage::Settings => "Settings",
                        }))
                        .when(!theme.classic, |heading| heading.child(div().text_sm().text_color(colors.muted_foreground).child(match self.page {
                            ControlPage::Captures => "Capture, edit and save screenshots.",
                            ControlPage::History => "Search and manage saved images.",
                            ControlPage::Sources => "Choose a display or window.",
                            ControlPage::Uploads => "Upload saved PNGs to your configured destination.",
                            ControlPage::Settings => "Appearance and storage.",
                        }))))
                    .child(quiet_button(cx, "appearance-shortcut", theme::name(theme::selected(cx)), false, false,
                        cx.listener(|this, _, _, cx| this.show_page(ControlPage::Settings, cx)))))
                .when(self.capture_delay.is_some(), |view| view.child(div().p_3().rounded(px(theme.radius)).bg(colors.button).flex().flex_wrap().items_center().gap_3()
                    .child("Screenshot scheduled · 3 second delay")
                    .child(capture_button(cx, "cancel-delayed-capture", "Cancel countdown", false,
                        cx.listener(|this, _, _, cx| this.cancel_capture_delay(cx))))))
            .when(self.page == ControlPage::Captures, |view| {
                view.child(capture_button(cx, "open-image", "Open image…", unavailable || self.preview_active,
                    cx.listener(|this, _, _, cx| this.open_image(cx))))
                .when(cfg!(target_os = "windows"), |view| view.child(capture_button(cx,
                    "paste-image", "Paste image (Ctrl+Shift+V)", unavailable || self.preview_active,
                    cx.listener(|this, _, _, cx| this.paste_image(cx)))))
                .child(div().text_sm().text_color(colors.muted_foreground)
                    .child("Drop one PNG, JPEG or WebP image into this window to edit a copy."))
                .when(self.capture_status != "Ready", |view| view.child(div().text_sm().child(self.capture_status.clone())))
            .when(theme.classic && self.last_capture.is_none() && self.last_saved.is_none(), |view| view.child(classic_capture_home(cx)))
            .when(!theme.classic && self.last_capture.is_none() && self.last_saved.is_none(), |view| view.child(
                div().flex_none().p_5().rounded(px(theme.panel_radius))
                    .border_1().border_color(colors.border).bg(colors.group_box)
                    .flex().flex_col().gap_3()
                    .child(div().text_lg().font_weight(gpui::FontWeight::MEDIUM).child("No screenshots yet"))
                    .child(div().text_sm().text_color(colors.muted_foreground).child("Capture a region to annotate, copy or save."))
                    .child(div().flex().flex_wrap().items_center().gap_3()
                        .child(ui_button(cx, "workspace-annotate", "Capture a region", unavailable, true,
                            cx.listener(|this, _, _, cx| this.select_region_to_edit(cx))))
                        .child(shortcut_hint("New screenshot", "n", cx)))
            ))
            .when(self.last_capture.is_some() || self.last_saved.is_some(), |view| {
                view            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .p_4()
                    .gap_2()
                    .border_1().border_color(colors.border)
                    .bg(colors.group_box)
                    .rounded(px(theme.panel_radius)).child(div().text_lg().font_weight(gpui::FontWeight::SEMIBOLD).child("Latest screenshot"))
                    .children(self.last_saved.as_ref().and_then(|path| path.file_name()).map(|name|
                        div().text_sm().text_color(colors.muted_foreground).child(name.to_string_lossy().into_owned())))
                    .children(
                        self.last_render
                            .clone()
                            .map(|render| img(render).h(px(240.0)).max_w_full()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(capture_button(cx,
                                "copy-last-capture",
                                "Copy image",
                                busy || (self.last_capture.is_none() && self.last_saved.is_none()),
                                cx.listener(|this, _, _, cx| this.copy_last_capture(cx)),
                            ))
                            .child(capture_button(cx,
                                "redact-last-capture",
                                "Edit a copy",
                                busy || (self.last_capture.is_none() && self.last_saved.is_none()),
                                cx.listener(|this, _, _, cx| this.open_redaction_editor(cx)),
                            ))
                            .child(capture_button(cx,
                                "save-capture-as",
                                "Save as (.png / .jpg / .webp)…",
                                busy || (self.last_capture.is_none() && self.last_saved.is_none()),
                                cx.listener(|this, _, _, cx| this.save_capture_as(cx)),
                            ))
                            .child(capture_button(cx,
                                "copy-last-file-path",
                                "Copy file path",
                                busy || self.preview_active || self.last_saved.is_none(),
                                cx.listener(|this, _, _, cx| this.copy_last_file_path(cx)),
                            ))
                            .child(capture_button(cx,
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
                    .child(div().text_sm().child("Save as size · custom outputs are copies; the current image and library stay unchanged. Exact dimensions can change the aspect ratio."))
                    .child(RadioGroup::new("save-as-size")
                        .children(["Original", "50%", "25%", "Custom width", "Custom height", "Exact dimensions"])
                        .selected_index(Some(match self.export_size { app_core::ExportSize::Original => 0,
                            app_core::ExportSize::Half => 1, app_core::ExportSize::Quarter => 2,
                            app_core::ExportSize::Width(_) => 3, app_core::ExportSize::Height(_) => 4, app_core::ExportSize::Exact(_, _) => 5 }))
                        .disabled(busy || self.preview_active)
                        .on_change(cx.listener(|this, index, _, cx| {
                            if let Some(size) = [app_core::ExportSize::Original, app_core::ExportSize::Half, app_core::ExportSize::Quarter, app_core::ExportSize::Width(640), app_core::ExportSize::Height(480), app_core::ExportSize::Exact(640, 480)].get(*index) {
                                this.export_size = *size; this.export_dimension_input = None; this.export_height_input = None; cx.notify();
                            }
                        })))
                    .children(self.export_dimension_input.iter().map(|input| div().flex().flex_col().gap_1()
                        .child(if matches!(self.export_size, app_core::ExportSize::Exact(_, _)) {
                            "Export width in physical pixels · independent of height; no upscaling"
                        } else if matches!(self.export_size, app_core::ExportSize::Height(_)) {
                            "Export height in physical pixels · width follows the aspect ratio; no upscaling"
                        } else { "Export width in physical pixels · height follows the aspect ratio; no upscaling" })
                        .child(gpui::component::input::Input::new(input).id("export-dimension")
                            .aria_label(if matches!(self.export_size, app_core::ExportSize::Height(_)) { "Export height in pixels" } else { "Export width in pixels" })
                            .disabled(busy || self.preview_active))))
                    .children(self.export_height_input.iter().map(|input| div().flex().flex_col().gap_1()
                        .child("Export height in physical pixels · independent of width; no upscaling")
                        .child(gpui::component::input::Input::new(input).id("export-height").aria_label("Export height in pixels").disabled(busy || self.preview_active))))
                    .child(div().text_sm().child(self.clipboard_status.clone()))
                    .child(div().text_sm().child(self.save_status.clone())),
            )
            })
            })
            .when(self.page == ControlPage::Captures && !theme.classic, |view| view
                .child(div().flex_none().p_2().rounded(px(theme.panel_radius)).border_1().border_color(colors.border)
                    .bg(colors.group_box).flex().flex_col().gap_1()
                    .child(capture_tile(cx, "select-region", "Quick region", self.output_profile.label(), unavailable,
                        cx.listener(|this, _, _, cx| this.open_overlays(OverlayMode::Fullscreen, cx))))
                    .when(cfg!(target_os = "windows"), |view| view.child(capture_tile(cx, "repeat-region", "Repeat last region", "Current session · unchanged display geometry required", unavailable || self.preview_active || self.last_region.is_none(),
                        cx.listener(|this, _, _, cx| this.repeat_last_region(cx)))))
                    .child(capture_tile(cx, "delayed-region", "3-second delay", "Wait before selecting a region", unavailable,
                        cx.listener(|this, _, _, cx| this.start_delayed_region(cx))))
                    .child(capture_tile(cx, "nav-sources", "Display or window", "Choose a capture source", unavailable,
                        cx.listener(|this, _, _, cx| this.show_page(ControlPage::Sources, cx))))))
            .when(self.history_export_cancel.is_some(), |view| {
                view.child(capture_button(cx,
                    "cancel-history-export",
                    "Cancel export",
                    self.history_export_cancel
                        .as_ref()
                        .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed)),
                    cx.listener(|this, _, _, cx| this.cancel_history_export(cx)),
                ))
            })
            .when(self.page == ControlPage::Uploads, |view| {
                view.child(div().flex_none().p_5().rounded(px(theme.panel_radius)).bg(colors.group_box).flex().flex_col().gap_4()
                    .child(div().flex().items_center().gap_3()
                        .child(div().w(px(40.)).h(px(40.)).rounded(px(theme.radius)).bg(colors.button).flex().items_center().justify_center().child(ui_icon(cx, "nav-uploads", 20.)))
                        .child(div().flex().flex_col().gap_1()
                            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Destination"))
                            .child(div().text_sm().text_color(colors.muted_foreground).child(if self.upload_destination.is_some() { "Ready for an explicit upload" } else { "Connect your own image hosting service" }))))
                    .child(div().text_sm().text_color(colors.muted_foreground).child("Load a destination JSON profile. Uploads only start when you choose an image and confirm."))
                    .child(div().flex().flex_wrap().gap_2()
                        .child(capture_button(cx, "load-upload-destination", if self.upload_destination.is_some() { "Change destination…" } else { "Choose destination…" },
                            self.upload_config_loading || self.upload_cancel.is_some(), cx.listener(|this, _, _, cx| this.load_upload_destination(cx))))
                        .when(self.upload_destination.is_some(), |row| row
                            .child(quiet_button(cx, "remember-upload-destination", "Remember", self.upload_config_loading || self.upload_cancel.is_some(), false,
                                cx.listener(|this, _, _, cx| this.persist_upload_destination(false, cx)))))
                        .child(quiet_button(cx, "forget-upload-destination", "Forget destination", self.upload_config_loading || self.upload_cancel.is_some(), false,
                            cx.listener(|this, _, _, cx| this.persist_upload_destination(true, cx)))))
                    .children(self.upload_destination.as_ref().and_then(|value| value.credential_target()).map(|target| {
                        div().pt_3().border_t_1().border_color(colors.border).flex().flex_col().gap_2()
                            .child(div().text_xs().text_color(colors.muted_foreground).child(format!("Credential Manager target: {target}")))
                            .child(quiet_button(cx, "copy-upload-credential-target", "Copy credential target", false, false,
                                cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(target.clone()));
                                    this.clipboard_status = "Credential target copied (no token)".into(); cx.notify();
                                })))
                    })))
                .child(div().flex_none().p_5().rounded(px(theme.panel_radius)).bg(colors.group_box).flex().flex_col().gap_3()
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Share your latest screenshot"))
                    .child(div().text_sm().text_color(colors.muted_foreground).child(
                        self.last_saved.as_ref().and_then(|path| path.file_name()).map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "Save a screenshot first, or choose an image from your library.".into())))
                    .child(div().flex().flex_wrap().gap_2()
                        .child(ui_button(cx, "upload-saved-png", "Upload image", busy || self.upload_config_loading || self.upload_cancel.is_some()
                            || self.upload_destination.is_none() || self.last_saved.is_none(), true,
                            cx.listener(|this, _, _, cx| this.start_upload(cx))))
                        .when(self.upload_cancel.is_some(), |row| row.child(capture_button(cx, "cancel-upload", "Cancel upload", false,
                            cx.listener(|this, _, _, cx| this.cancel_upload(cx)))))
                        .when(self.upload_url.is_some(), |row| row.child(capture_button(cx, "copy-upload-link", "Copy link", false,
                            cx.listener(|this, _, _, cx| {
                                if let Some(url) = &this.upload_url {
                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(url.clone()));
                                    this.upload_status = "Upload link copied".into();
                                    this.clipboard_status = "Upload link copied to clipboard".into(); cx.notify();
                                }
                            })))))
                    .child(div().text_sm().child(self.upload_status.clone()))
                    .children(self.upload_url.clone().map(|url| div().text_sm().text_color(colors.caret).child(url))))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(capture_button(cx, "toggle-upload-history", if self.upload_history_visible {"Hide upload history"} else {"Upload history"}, false,
                        cx.listener(|this,_,_,cx| { if this.upload_history_visible {this.hide_upload_history();} else {this.upload_history_visible=true;this.refresh_upload_history(None,cx);} cx.notify(); })))
                    .when(self.upload_history_visible, |view| {
                        let revision = self.upload_history_revision;
                        view.child(div().text_sm().child(self.upload_history_status.clone()))
                            .child(div().id("upload-history-list").max_h(px(320.0)).overflow_y_scroll().flex().flex_col().gap_2()
                                .children(self.upload_history_entries.clone().into_iter().map(|entry| {
                                    let id=entry.id; let url=entry.url.clone();
                                    div().id(gpui::SharedString::from(format!("upload-history-{}", entry.id))).p_2().bg(colors.button)
                                        .child(entry.filename).child(div().text_sm().child(format!("{} · {}",entry.origin,entry.created_at)))
                                        .child(div().text_sm().child(entry.url))
                                        .child(div().flex().gap_2()
                                            .child(capture_button(cx, "copy-upload-history-link","Copy link",false,cx.listener(move |this,_,_,cx| {
                                                if this.upload_history_visible && this.upload_history_revision==revision && this.upload_history_entries.iter().any(|entry| entry.id==id) {
                                                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(url.clone())); this.clipboard_status="Upload history link copied".into(); cx.notify();
                                                }
                                            })))
                                            .child(capture_button(cx, "remove-upload-history-link","Remove from library",self.upload_history_loading || self.history_mutating,cx.listener(move |this,_,_,cx|this.remove_upload_entry(id,revision,cx)))))
                                })))
                            .child(div().flex().gap_2()
                                .child(capture_button(cx, "upload-history-latest","Latest",self.upload_history_loading,cx.listener(|this,_,_,cx|this.refresh_upload_history(None,cx))))
                                .child(capture_button(cx, "upload-history-older","Older",self.upload_history_loading || !self.upload_history_more,cx.listener(|this,_,_,cx| {
                                    if let Some(entry)=this.upload_history_entries.last() {this.refresh_upload_history(Some(entry.id),cx);}
                                }))))
                    }),
            )
            })
            .when(self.page == ControlPage::Sources, |view| {
                view.when(self.backend.capabilities().capture_region, |view| view.child(
                    div().flex().flex_col().gap_2()
                        .child("Exact region")
                        .child(div().text_sm().child("Enter physical pixels relative to the chosen display's top-left corner. Then choose Exact region beside that display."))
                        .children(self.exact_region_inputs.iter().map(|inputs| div().flex().flex_wrap().gap_2()
                            .children(inputs.iter().enumerate().map(|(index, input)| div().w(px(120.)).flex().flex_col().gap_1()
                                .child(["X", "Y", "Width", "Height"][index])
                                .child(gpui::component::input::Input::new(input).id(("exact-region-input", index))
                                    .aria_label(["Region X", "Region Y", "Region width", "Region height"][index]).disabled(unavailable || self.refreshing))))))
                        .children(self.region_preset_name.iter().map(|input| gpui::component::input::Input::new(input)
                            .id("region-preset-name").aria_label("Region preset name").disabled(unavailable || self.refreshing)))
                        .child(div().text_sm().child("Save up to 16 named rectangles. Saving the same name replaces it. Load a preset, then explicitly choose the target display."))
                        .children(self.region_presets.iter().cloned().enumerate().map(|(index, preset)| {
                            let remove = preset.clone();
                            div().flex().flex_wrap().items_center().gap_2()
                                .child(format!("{} · {}, {} · {} × {}", preset.name, preset.x, preset.y, preset.width, preset.height))
                                .child(Button::new(("use-region-preset", index)).small().label("Use").disabled(unavailable)
                                    .accessibility_label(format!("Use region preset {}", preset.name))
                                    .on_click(cx.listener(move |this, _, window, cx| this.use_region_preset(&preset, window, cx))))
                                .child(Button::new(("remove-region-preset", index)).small().label("Remove").disabled(unavailable)
                                    .accessibility_label(format!("Remove region preset {}", remove.name))
                                    .on_click(cx.listener(move |this, _, _, cx| this.remove_region_preset(&remove, cx))))
                        }))
                        .child(div().text_sm().child(self.exact_region_status.clone()))))
                .child(
                div().flex().flex_col().gap_2()
                    .child("Capture a display")
                    .children(displays.into_iter().map(|display| {
                        let id = display.id.clone();
                        let exact_display = display.clone();
                        div().flex().flex_wrap().gap_2().child(Button::new(gpui::SharedString::from(format!("display-capture-{}", id)))
                            .small().w_full().justify_start().icon(ui_icon(cx, "nav-sources", 16.))
                            .disabled(unavailable).label(format!("{} · {} × {}{}", display.name,
                                display.width, display.height, if display.is_primary { " · Primary display" } else { "" }))
                            .on_click(cx.listener(move |this, _, _, cx| this.start_capture(vec![CaptureJob::Display(id.clone())], cx))))
                            .when(self.backend.capabilities().capture_region, |row| row.child(Button::new(
                                gpui::SharedString::from(format!("exact-region-{}", display.id)))
                                .small().label("Exact region").accessibility_label(format!("Capture exact region on {}", display.name))
                                .disabled(unavailable || self.refreshing)
                                .on_click(cx.listener(move |this, _, _, cx| this.capture_exact_region(exact_display.clone(), cx)))))
                            .when(self.backend.capabilities().capture_region, |row| row.child(Button::new(
                                gpui::SharedString::from(format!("save-region-preset-{}", display.id)))
                                .small().label("Save / replace preset").disabled(unavailable || self.refreshing)
                                .accessibility_label(format!("Save region preset for {}", display.name))
                                .on_click(cx.listener(move |this, _, _, cx| this.save_region_preset(display.clone(), cx)))))
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
                                .child(capture_button(cx,
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
                                .children(windows.into_iter().map(
                                    |(id, title)| {
                                        Button::new(gpui::SharedString::from(format!("capture-window-{}", id)))
                                            .small().w_full().justify_start().icon(ui_icon(cx, "nav-sources", 16.))
                                            .disabled(unavailable).label(title)
                                            .on_click(cx.listener(move |this, _, _, cx| this.start_capture(vec![CaptureJob::Window(id.clone())], cx)))
                                    },
                                )),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(colors.muted_foreground)
                                .child(self.list_status.clone()),
                        ),
                )
            })
            })
            .when(self.page == ControlPage::History, |view| { view
            .when(self.history_visible, |view| {
                let revision = self.history_revision;
                view.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()


                        .children(self.history_search_input.as_ref().map(|input| gpui::component::input::Input::new(input).id("library-search").aria_label("Search library").disabled(self.history_mutating)))
                        .child(
                            div().flex().flex_wrap().gap_2().children(
                                [
                                    (
                                        "history-filter-all",
                                        "All",
                                        "All",
                                        app_core::HistoryFilter::All,
                                    ),
                                    (
                                        "history-filter-captures",
                                        "Captures",
                                        "Captures",
                                        app_core::HistoryFilter::Captures,
                                    ),
                                    (
                                        "history-filter-imports",
                                        "Imports",
                                        "Imports",
                                        app_core::HistoryFilter::Imports,
                                    ),
                                ]
                                .into_iter()
                                .map(
                                    |(id, label, active, filter)| {
                                        quiet_button(cx,
                                            id,
                                            if self.history_filter == filter {
                                                active
                                            } else {
                                                label
                                            },
                                            self.history_mutating,
                                            self.history_filter == filter,
                                            cx.listener(move |this, _, _, cx| {
                                                this.set_history_filter(filter, cx)
                                            }),
                                        )
                                    },
                                ),
                            )
                        .child(quiet_button(cx, "library-open-image", "Open image…", unavailable || self.preview_active, false,
                            cx.listener(|this, _, _, cx| this.open_image(cx))))
                        .child(quiet_button(cx,
                            "import-history-images",
                            "Add PNGs…",
                            busy || self.history_loading,
                            false, cx.listener(|this, _, _, cx| this.import_history_images(cx)),
                        ))
                                .child(quiet_button(cx,
                                    "select-history-page",
                                    "Select all",
                                    self.history_loading
                                        || self.history_mutating
                                        || self.history_entries.is_empty(),
                                    false, cx.listener(move |this, _, _, cx| {
                                        this.select_history_page(revision, cx)
                                    }),
                                )),
                        )
                        .child(div().text_xs().text_color(colors.muted_foreground).child(self.history_status.clone()))


                        .when(!self.history_action_status.is_empty(), |view| view.child(div().text_sm().child(self.history_action_status.clone())))
                        .when(!self.history_preview_status.is_empty(), |view| view.child(div().text_sm().child(self.history_preview_status.clone())))
                        .when(!self.history_selected.is_empty(), |view| view.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_1().items_center()
                                .child(div().px_2().text_xs().text_color(colors.muted_foreground).child(format!("{} selected", self.history_selected.len())))
                                .child(quiet_button(cx,
                                    "clear-history-selection",
                                    "Clear selection",
                                    self.history_mutating || self.history_selected.is_empty(),
                                    false, cx.listener(move |this, _, _, cx| {
                                        if this.history_visible
                                            && !this.history_mutating
                                            && !this.history_loading
                                            && this.history_revision == revision
                                        {
                                            this.history_selected.clear();
                                            cx.notify();
                                        }
                                    }),
                                ))
                                .child(quiet_button(cx,
                                    "export-history-selection",
                                    "Export selected…",
                                    busy || self.history_loading
                                        || self.history_selected.is_empty(),
                                    false, cx.listener(move |this, _, _, cx| {
                                        this.export_selected_history(revision, cx)
                                    }),
                                ))
                                .child(quiet_button(cx,
                                    "remove-history-selection",
                                    "Remove selected",
                                    busy || self.history_loading
                                        || self.history_selected.is_empty(),
                                    false, cx.listener(move |this, _, _, cx| {
                                        if this.history_revision == revision {
                                            this.remove_history_captures(
                                                this.history_selected.clone(),
                                                cx,
                                            );
                                        }
                                    }),
                                ))
                                ,
                        ))
                        .when(!self.history_loading && self.history_entries.is_empty(), |view| view.child(
                            div().p_6().rounded(px(theme.panel_radius)).border_1().border_color(colors.border).bg(colors.group_box)
                                .child(div().text_lg().child("No images to show"))
                                .child(div().mt_2().text_sm().text_color(colors.muted_foreground).child("Take a screenshot, add PNGs, or try a different search."))))
                        .child(
                            div()
                                .id("capture-history-list")
                                .flex()
                                .flex_col()
                                .gap_2()
                                .children(
                                    self.history_entries.clone().into_iter().map(
                                        |entry| {
                                                                                                                                    let copy_path = entry.path.clone();
                                            let edit_path = entry.path.clone();
                                            let entry_id = entry.id;
                                            let preview = self
                                                .history_preview
                                                .as_ref()
                                                .filter(|(id, _)| *id == entry_id)
                                                .map(|(_, render)| render.clone());
                                            let selected = self.history_selected.contains(&entry_id);
                                            let filename = entry.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                                            div().id(gpui::SharedString::from(format!("history-entry-{}", entry.id))).flex_none().flex().flex_col().gap_3()
                                                .p_3().rounded(px(theme.panel_radius)).bg(colors.group_box)
                                                .when(theme.classic, |row| row.p_2().gap_1())
                                                .border_1().border_color(if selected { colors.caret } else { colors.border })
                                                .child(div().flex().flex_wrap().items_center().gap_2()
                                                    .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                                                        .child(Button::new("history-preview").ghost().small().w_full().justify_start()
                                                            .accessibility_label(format!("Preview {filename}"))
                                                            .child(div().flex().items_center().gap_2().w_full().min_w_0()
                                                                .child(ui_icon(cx, "nav-captures", 16.))
                                                                .child(div().flex_1().min_w_0().text_ellipsis().child(filename)))
                                                            .disabled(preview.is_none() && (busy || self.history_loading))
                                                            .on_click(cx.listener(move |this, _, _, cx| this.preview_history_image(entry_id, revision, cx))))
                                                        .child(div().text_xs().text_color(colors.muted_foreground)
                                                            .child(format!("{} × {} · {}", entry.width, entry.height, entry.created_at))))
                                                    .child(div().flex().flex_wrap().gap_1()
                                                        .child(gpui::component::checkbox::Checkbox::new("select-history-entry")
                                                            .label("Select").checked(selected).small()
                                                            .disabled(self.history_loading || self.history_mutating)
                                                            .on_change(cx.listener(move |this, checked, _, cx| {
                                                                if this.history_selected.contains(&entry_id) != *checked {
                                                                    this.toggle_history_selection(entry_id, revision, cx);
                                                                }
                                                            })))
                                                        .child(quiet_button(cx, "edit-history-file", "Edit", busy, false,
                                                            cx.listener(move |this, _, _, cx| this.open_history_editor(edit_path.clone(), cx))))
                                                        .child(quiet_button(cx, "copy-history-file", "Copy", busy, false,
                                                            cx.listener(move |this, _, _, cx| this.copy_history_capture(copy_path.clone(), cx))))
                                                        .child(self.history_menu_button(entry_id, revision, self.history_loading || self.history_mutating, cx))))
                                                .children(preview.map(|render| div().p_3().rounded(px(theme.radius)).bg(colors.background)
                                                    .child(img(render).max_w_full().max_h(px(280.)))
                                                    .child(div().mt_2().text_xs().text_color(colors.muted_foreground).child("Click the image name to close this preview."))))

                                        },
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(quiet_button(cx,
                                    "history-latest",
                                    "Latest",
                                    self.history_loading || self.history_mutating,
                                    false, cx.listener(|this, _, _, cx| this.refresh_history(None, cx)),
                                ))
                                .child(quiet_button(cx,
                                    "history-older",
                                    "Older",
                                    self.history_loading
                                        || self.history_mutating
                                        || !self.history_has_more,
                                    false, cx.listener(|this, _, _, cx| {
                                        if let Some(entry) = this.history_entries.last() {
                                            this.refresh_history(Some(entry.id), cx);
                                        }
                                    }),
                                )),
                        ),
                )
            })
            })
            .when(self.page == ControlPage::Settings, |view| {
                view.child(
                div().flex_none().p_5().rounded(px(theme.panel_radius)).border_1().border_color(colors.border)
                    .bg(colors.group_box).flex().flex_col().gap_4()
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Appearance"))
                    .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Choose an appearance. Changes are saved automatically."))
                    .child(RadioGroup::new("appearance").layout(gpui::Axis::Vertical)
                        .children(["Dark (default)", "Oldschool"])
                        .selected_index(Some(if theme::selected(cx) == app_core::UiTheme::Default { 0 } else { 1 }))
                        .disabled(self.settings_loading || self.settings_saving || self.output_directory.is_none())
                        .on_change(cx.listener(|this, ix, _, cx| this.choose_theme(
                            if *ix == 0 { app_core::UiTheme::Default } else { app_core::UiTheme::Oldschool }, cx))))
                    .child(div().text_sm().text_color(colors.muted_foreground).child(self.settings_status.clone()))
            ).child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .p_5()
                    .gap_3()
                    .bg(colors.group_box)
                    .rounded(px(theme.panel_radius)).child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Storage"))
                    .child(div().text_sm().text_color(colors.muted_foreground).child("Save folder"))
                    .child(div().text_sm().child(folder))
                    .child(capture_button(cx,
                        "choose-output-directory",
                        "Change folder…",
                        busy,
                        cx.listener(|this, _, _, cx| this.choose_output_directory(cx)),
                    ))
                    .child(div().text_xs().text_color(colors.muted_foreground).child("Existing files stay where they are.")),
            )
            .child(div().flex_none().p_5().rounded(px(theme.panel_radius)).bg(colors.group_box).flex().flex_col().gap_3()
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Capture output"))
                .child(div().text_sm().child("Applies to quick region, delayed region, display and window captures, including tray and global shortcuts. New screenshot always opens the editor. Changes are saved automatically."))
                .child(RadioGroup::new("capture-output-profile").layout(gpui::Axis::Vertical)
                    .children(CaptureOutputProfile::LABELS)
                    .selected_index(CaptureOutputProfile::ALL.iter().position(|profile| *profile == self.output_profile))
                    .disabled(busy || self.preview_active || self.output_directory.is_none())
                    .on_change(cx.listener(|this, ix, _, cx| {
                        if let Some(profile) = CaptureOutputProfile::ALL.get(*ix) { this.choose_output_profile(*profile, cx); }
                    })))
                .child(div().text_sm().text_color(colors.muted_foreground).child("Copy only keeps one image in memory without saving. Annotate first produces no output until you choose Copy or Save in the editor. With either option, capture one display or window at a time. Uploads always require a separate action.")))
            .child(div().flex_none().p_5().rounded(px(theme.panel_radius)).bg(colors.group_box).flex().flex_col().gap_3()
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("JPEG export quality"))
                .child(div().text_sm().child(format!("Active quality: {} · Applies only to JPEG copies created with Save as. PNG and lossless WebP are unchanged. JPEG requires opaque pixels.", self.jpeg_quality)))
                .children(self.jpeg_quality_input.iter().map(|input| gpui::component::input::Input::new(input)
                    .id("jpeg-export-quality").aria_label("JPEG quality (1–100)").disabled(unavailable || self.preview_active)))
                .child(capture_button(cx, "save-jpeg-quality", "Save quality (1–100)", unavailable || self.preview_active,
                    cx.listener(|this, _, _, cx| this.save_jpeg_quality(cx)))))
            .child(div().flex_none().p_5().rounded(px(theme.panel_radius)).bg(colors.group_box).flex().flex_col().gap_3()
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Default Save as format"))
                .child(RadioGroup::new("save-as-format").layout(gpui::Axis::Vertical)
                    .children(app_core::ImageExportFormat::ALL.map(app_core::ImageExportFormat::label))
                    .selected_index(app_core::ImageExportFormat::ALL.iter().position(|format| *format == self.export_format))
                    .disabled(unavailable || self.preview_active)
                    .on_change(cx.listener(|this, index, _, cx| {
                        if let Some(format) = app_core::ImageExportFormat::ALL.get(*index) { this.choose_export_format(*format, cx); }
                    })))
                .child(div().text_sm().child("Changes are saved automatically. Save as suggests this format and uses it when the filename has no extension. An explicit extension overrides it. Direct Save and quick captures still use PNG.")))
            .child(div().flex_none().p_5().rounded(px(theme.panel_radius)).bg(colors.group_box).flex().flex_col().gap_3()
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Capture filename template"))
                .child(div().text_sm().child("Automatic PNG saves only, including edited captures. Use letters, digits, spaces, - or _, and {date} / {time} for UTC save time. A unique suffix and .png are always added. Leave empty for default names; Save as keeps its manually chosen name."))
                .child(div().text_sm().child(if self.filename_template.is_empty() { "Active: default names".to_owned() } else { format!("Active: {}", self.filename_template) }))
                .children(self.filename_template_input.iter().map(|input| gpui::component::input::Input::new(input)
                    .id("capture-filename-template").aria_label("Capture filename template").disabled(unavailable || self.preview_active)))
                .child(capture_button(cx, "save-filename-template", "Save filename template", unavailable || self.preview_active,
                    cx.listener(|this, _, _, cx| this.save_filename_template(cx)))))
            .child(self.hotkey_settings(cx))
            .child(capture_button(cx,
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
                        .text_color(colors.muted_foreground)
                        .child(self.hotkey_status.clone())
                        .child(self.tray_status.clone())
                        .child(format!("{} overlay window(s)", self.overlays.len()))
                        .child(capture_button(cx,
                            "open-probe",
                            "Open test overlay",
                            unavailable,
                            cx.listener(|this, _, _, cx| {
                                this.open_overlays(OverlayMode::Probe, cx)
                            }),
                        ))
                        .child(capture_button(cx,
                            "close-overlays",
                            "Close overlays",
                            false,
                            cx.listener(|this, _, _, cx| this.close_overlays(cx)),
                        )),
                )
            })
            })
            ))
    }
}
impl CaptureWorkspace {
    fn hotkey_settings(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let panel = div().flex_none().p_5().flex().flex_col().gap_3()
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Global shortcuts"));
        #[cfg(not(target_os = "windows"))]
        { panel.child("Global shortcut configuration is currently available on Windows only. Use your desktop's shortcut settings where supported.").into_any_element() }
        #[cfg(target_os = "windows")]
        {
            let disabled = self.settings_loading || self.settings_saving || self.quitting || self.hotkey.is_none() || self.output_directory.is_none();
            panel.child(div().text_sm().child("Use Ctrl, Alt, Shift or Super (Windows key) + a letter, digit, F1–F24 or PrintScreen. Letters/digits need Ctrl, Alt or Super. Leave empty to disable."))
                .children(self.hotkey_inputs.iter().flat_map(|inputs| inputs.iter().enumerate()).map(|(index, input)| {
                    div().flex().flex_col().gap_1()
                        .child(app_core::CaptureHotkeys::LABELS[index])
                        .child(gpui::component::input::Input::new(input)
                            .id(("capture-hotkey", index))
                            .aria_label(app_core::CaptureHotkeys::LABELS[index])
                            .disabled(disabled))
                }))
                .child(div().flex().flex_wrap().gap_2()
                    .child(capture_button(cx, "save-hotkeys", "Save shortcuts", disabled,
                        cx.listener(|this, _, _, cx| {
                            if let Some(inputs) = &this.hotkey_inputs {
                                let bindings = app_core::CaptureHotkeys(std::array::from_fn(|i| inputs[i].read(cx).value().to_string()));
                                this.save_hotkeys(bindings, cx);
                            }
                        })))
                    .child(capture_button(cx, "restore-hotkeys", "Restore defaults", disabled,
                        cx.listener(|this, _, _, cx| this.save_hotkeys(app_core::CaptureHotkeys::default(), cx)))))
                .child(div().text_sm().child(format!("Active: {}", self.hotkey_status)))
                .into_any_element()
        }
    }

    fn render_sidebar(
        &self,
        unavailable: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let classic = theme::current(cx).classic;
        let navigation = SidebarMenu::new()
            .child(
                SidebarMenuItem::new("Capture")
                    .icon(ui_icon(cx, "capture-options", 16.))
                    .click_to_toggle(true)
                    .children([
                        SidebarMenuItem::new("Quick region")
                            .icon(ui_icon(cx, "select-region", 16.))
                            .disable(unavailable)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_overlays(OverlayMode::Fullscreen, cx)
                            })),
                        SidebarMenuItem::new("3 second delay")
                            .icon(ui_icon(cx, "delayed-region", 16.))
                            .disable(unavailable)
                            .on_click(cx.listener(|this, _, _, cx| this.start_delayed_region(cx))),
                        SidebarMenuItem::new("Primary display")
                            .icon(ui_icon(cx, "capture-primary", 16.))
                            .disable(unavailable)
                            .on_click(cx.listener(|this, _, _, cx| this.capture_primary(cx))),
                        #[cfg(target_os = "windows")]
                        SidebarMenuItem::new("Repeat last region")
                            .icon(ui_icon(cx, "select-region", 16.))
                            .disable(unavailable || self.preview_active || self.last_region.is_none())
                            .on_click(cx.listener(|this, _, _, cx| this.repeat_last_region(cx))),
                        #[cfg(target_os = "windows")]
                        SidebarMenuItem::new("Active window in 3 seconds")
                            .icon(ui_icon(cx, "capture-primary", 16.))
                            .disable(unavailable)
                            .on_click(cx.listener(|this, _, _, cx| this.start_delayed_capture(DelayedCapture::ActiveWindow, cx))),
                        SidebarMenuItem::new("All displays")
                            .icon(ui_icon(cx, "capture-all", 16.))
                            .disable(unavailable || (self.output_profile.requires_single_image() && self.xcap_displays.len() > 1))
                            .on_click(cx.listener(|this, _, _, cx| this.capture_all(cx))),
                    ]),
            )
            .child(
                SidebarMenuItem::new("Recent capture")
                    .icon(ui_icon(cx, "nav-captures", 16.))
                    .active(self.page == ControlPage::Captures)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.show_page(ControlPage::Captures, cx)),
                    ),
            )
            .child(
                SidebarMenuItem::new("Library")
                    .icon(ui_icon(cx, "nav-library", 16.))
                    .active(self.page == ControlPage::History)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.show_page(ControlPage::History, cx)),
                    ),
            )
            .child(
                SidebarMenuItem::new("Displays & windows")
                    .icon(ui_icon(cx, "nav-sources", 16.))
                    .active(self.page == ControlPage::Sources)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.show_page(ControlPage::Sources, cx)),
                    ),
            )
            .child(
                SidebarMenuItem::new("Uploads")
                    .icon(ui_icon(cx, "nav-uploads", 16.))
                    .active(self.page == ControlPage::Uploads)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.show_page(ControlPage::Uploads, cx)),
                    ),
            );
        let footer = SidebarMenu::new()
            .child(
                SidebarMenuItem::new("Settings")
                    .icon(ui_icon(cx, "nav-settings", 16.))
                    .active(self.page == ControlPage::Settings)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.show_page(ControlPage::Settings, cx)),
                    ),
            )
            .child(
                SidebarMenuItem::new("Open folder")
                    .icon(ui_icon(cx, "sidebar-folder", 16.))
                    .disable(self.output_directory.is_none())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(path) = &this.output_directory {
                            cx.reveal_path(path);
                        }
                    })),
            )
            .child(
                SidebarMenuItem::new("Quit")
                    .icon(ui_icon(cx, "sidebar-quit", 16.))
                    .on_click(|_, _, cx| cx.defer(close_application)),
            )
            .render("workspace-footer", window, cx)
            .into_any_element();
        Sidebar::new("workspace-sidebar")
            .collapsible(false)
            .h_full()
            .w(gpui::rems(13.))
            .when(classic, |sidebar| sidebar.w(gpui::rems(12.5)))
            .header(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when(!classic, |header| {
                        header.child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("screenshots"),
                        )
                    })
                    .child(
                        ui_button(
                            cx,
                            "new-capture",
                            if classic {
                                "Capture & annotate"
                            } else {
                                "New screenshot"
                            },
                            unavailable,
                            false,
                            cx.listener(|this, _, _, cx| this.select_region_to_edit(cx)),
                        )
                        .w_full(),
                    ),
            )
            .child(navigation)
            .footer(footer)
    }

    fn history_menu_button(
        &self,
        id: i64,
        revision: u64,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let owner = cx.entity().downgrade();
        Button::new("history-more-actions")
            .ghost()
            .small()
            .label("More")
            .icon(ui_icon(cx, "history-more-actions", 16.))
            .disabled(disabled)
            .dropdown_menu(move |menu, _, cx| {
                let available = owner
                    .upgrade()
                    .map(|app| {
                        let app = app.read(cx);
                        (
                            !app.capturing && !app.copying && !app.saving && !app.quitting,
                            app.upload_destination.is_some()
                                && app.upload_cancel.is_none()
                                && !app.upload_config_loading,
                        )
                    })
                    .unwrap_or((false, false));
                [
                    ("Open", HistoryCommand::Open, false),
                    ("Show in folder", HistoryCommand::Reveal, false),
                    ("Copy file path", HistoryCommand::CopyPath, !available.0),
                    (
                        "Upload PNG",
                        HistoryCommand::Upload,
                        !available.0 || !available.1,
                    ),
                    ("Relink PNG…", HistoryCommand::Relink, !available.0),
                    ("Remove from library", HistoryCommand::Remove, !available.0),
                ]
                .into_iter()
                .fold(menu, |menu, (label, command, disabled)| {
                    let owner = owner.clone();
                    menu.item(PopupMenuItem::new(label).disabled(disabled).on_click(
                        move |_, _, cx| {
                            owner
                                .update(cx, |this, cx| {
                                    this.run_history_command(id, revision, command, cx)
                                })
                                .ok();
                        },
                    ))
                })
            })
    }
}

fn key_hint(key: &str, cx: &App) -> gpui::component::kbd::Kbd {
    let key = format!(
        "{}-{key}",
        if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        }
    );
    gpui::component::kbd::Kbd::new(
        gpui::Keystroke::parse_with_os(&key, cx.platform_info().os)
            .expect("fixed application shortcut"),
    )
}

fn classic_capture_home(cx: &App) -> impl IntoElement {
    let colors = cx.theme().colors;
    div()
        .flex_none()
        .self_center()
        .w_full()
        .max_w(gpui::rems(30.))
        .mt_16()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child("Capture shortcuts"),
        )
        .child(
            div()
                .text_sm()
                .text_color(colors.muted_foreground)
                .child("Use Capture on the left or a shortcut to get started."),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(colors.border)
                .children(
                    [
                        ("n", "Capture & annotate"),
                        ("1", "Recent capture"),
                        ("2", "Library"),
                        ("3", "Displays & windows"),
                        (",", "Settings"),
                    ]
                    .into_iter()
                    .map(move |(key, label)| {
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .py_2()
                            .text_sm()
                            .child(div().w_24().child(key_hint(key, cx)))
                            .child(label)
                    }),
                ),
        )
}

fn shortcut_hint(label: &'static str, key: &'static str, cx: &App) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(label)
        .child(key_hint(key, cx))
}

fn capture_tile(
    cx: &App,
    id: &'static str,
    label: &'static str,
    description: &'static str,
    disabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .py_1()
        .child(
            Button::new(id)
                .ghost()
                .small()
                .label(label)
                .icon(ui_icon(cx, id, 16.))
                .disabled(disabled)
                .w_48()
                .justify_start()
                .on_click(on_click),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(description),
        )
}

pub(super) fn ui_icon(cx: &App, id: &'static str, edge: f32) -> Icon {
    use gpui::assets::IconName as Glyph;
    let glyph = match id {
        "select-region" | "repeat-region" | "workspace-annotate" => Glyph::Crop,
        "select-region-to-edit" | "capture-options" => Glyph::Camera,
        "delayed-region" => Glyph::Timer,
        "nav-captures" | "preview-history-file" => Glyph::Image,
        "nav-library" => Glyph::Library,
        "nav-sources" | "capture-primary" | "capture-all" => Glyph::Monitor,
        "nav-uploads" => Glyph::Upload,
        "nav-settings" => Glyph::Settings,
        "sidebar-folder" => Glyph::FolderOpen,
        "sidebar-quit" => Glyph::LogOut,
        "copy-history-file" => Glyph::Copy,
        "edit-history-file" => Glyph::Pencil,
        "history-more-actions" => Glyph::Ellipsis,
        "select-history-entry" => Glyph::Check,
        "unselected-history-entry" => Glyph::Square,
        "appearance-shortcut" => Glyph::Palette,
        "tool-select" => Glyph::MousePointer,
        "tool-crop" => Glyph::Crop,
        "tool-frame" => Glyph::Square,
        "tool-ellipse" => Glyph::Circle,
        "tool-line" => Glyph::Minus,
        "tool-arrow" => Glyph::ArrowUpRight,
        "tool-pen" => Glyph::Pencil,
        "tool-text" => Glyph::Type,
        "tool-number" => Glyph::ListOrdered,
        "tool-highlight" => Glyph::Highlighter,
        "tool-spotlight" => Glyph::Focus,
        "tool-redact" => Glyph::Shield,
        "tool-pixelate" => Glyph::Grid2x2,
        "tool-blur" => Glyph::Droplet,
        _ => Glyph::Image,
    };
    Icon::new(glyph)
        .size(gpui::rems(edge / 16.))
        .text_color(theme::icon_color(cx, id))
}

fn quiet_button(
    cx: &App,
    id: &'static str,
    label: &'static str,
    disabled: bool,
    selected: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    Button::new(id)
        .ghost()
        .small()
        .label(label)
        .disabled(disabled)
        .selected(selected)
        .when(
            matches!(
                id,
                "preview-history-file"
                    | "copy-history-file"
                    | "edit-history-file"
                    | "history-more-actions"
                    | "select-history-entry"
                    | "appearance-shortcut"
            ),
            |button| {
                button.icon(ui_icon(
                    cx,
                    if id == "select-history-entry" && !selected {
                        "unselected-history-entry"
                    } else {
                        id
                    },
                    16.,
                ))
            },
        )
        .on_click(on_click)
}

pub(super) fn capture_button(
    cx: &App,
    id: &'static str,
    label: &'static str,
    disabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    quiet_button(cx, id, label, disabled, false, on_click)
}

pub(super) fn ui_button(
    _cx: &App,
    id: &'static str,
    label: &'static str,
    disabled: bool,
    accented: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    Button::new(id)
        .small()
        .label(label)
        .disabled(disabled)
        .when(accented, |button| button.primary())
        .on_click(on_click)
}
