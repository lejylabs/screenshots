use super::*;
use gpui::test::{TestQueryExt, TestWindowExt};
use gpui::{AppContext, TestAppContext, px, rgb};

fn shortcut(key: &str) -> String {
    format!(
        "{}-{key}",
        if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        }
    )
}

#[gpui::test]
fn transparent_overlays_keep_their_alpha_surface_across_theme_changes(cx: &mut TestAppContext) {
    struct OverlayContent;
    impl Render for OverlayContent {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }
    cx.update(|cx| {
        let handle = cx
            .open_kit_window(
                WindowOptions {
                    window_background: WindowBackgroundAppearance::Transparent,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| OverlayContent),
            )
            .unwrap();
        let window: AnyWindowHandle = handle.into();
        for appearance in [
            app_core::UiTheme::Oldschool,
            app_core::UiTheme::Default,
            app_core::UiTheme::Classic,
        ] {
            theme::apply(appearance, cx);
            window
                .update(cx, |_, window, cx| {
                    window.render_frame(cx);
                    gpui::base::Root::update(window, cx, |root, _, _| {
                        assert_eq!(root.style().background, Some(gpui::rgba(0).into()));
                    });
                })
                .unwrap();
        }
    });
}

#[gpui::test]
fn classic_appearance_projects_one_dark_onto_kit_and_does_not_stick_to_dark(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        theme::init(cx);
        theme::apply(app_core::UiTheme::Classic, cx);
        let theme = gpui::component::Theme::global(cx);
        assert_eq!(theme.colors.background, rgb(0x282c33).into());
        assert_eq!(theme.colors.sidebar, rgb(0x2f343e).into());
        assert_eq!(theme.colors.popover, rgb(0x2f343e).into());
        assert_eq!(theme.colors.title_bar, rgb(0x3b414d).into());
        assert_eq!(theme.colors.status_bar, rgb(0x3b414d).into());
        assert_eq!(theme.colors.tab_active, rgb(0x282c33).into());
        assert_eq!(theme.colors.primary, rgb(0x74ade8).into());
        assert_eq!(theme.colors.caret, rgb(0x74ade8).into());
        assert_eq!(theme.colors.list_active_border, rgb(0x74ade8).into());
        assert_eq!(theme.colors.danger, rgb(0xd07277).into());
        assert_eq!(theme.window_radius, px(10.));
        assert_eq!(theme.font_size, px(16.));
        assert_eq!(theme.radius, px(4.));
        assert_eq!(theme.radius_lg, px(4.));
        assert!(theme.shadow && theme.focus_ring);
        assert_eq!(theme.highlight_theme.name, "Classic");
        assert_eq!(
            theme.highlight_theme.style.editor_background,
            Some(rgb(0x282c33).into())
        );
        theme::apply(app_core::UiTheme::Default, cx);
        let theme = gpui::component::Theme::global(cx);
        assert_eq!(theme.colors.background, rgb(0x141414).into());
        assert_eq!(theme.colors.primary, rgb(0xdedede).into());
        assert_eq!(theme.colors.title_bar, rgb(0x1b1b1b).into());
        assert_eq!(theme.font_size, px(16.));
        assert_ne!(theme.window_radius, px(10.));
        assert_ne!(theme.highlight_theme.name, "Classic");
        theme::apply(app_core::UiTheme::Oldschool, cx);
        let theme = gpui::component::Theme::global(cx);
        assert_eq!(theme.colors.title_bar, rgb(0x282d35).into());
    });
}

#[gpui::test]
fn classic_workbench_lays_out_title_tabs_and_status(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let window: AnyWindowHandle = handle.into();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.find("title-bar");
        assert!(window.try_find("workbench-tab-bar").is_none());
        #[cfg(target_os = "windows")]
        {
            window.find("window-controls");
            window.find("close");
        }
        theme::init(cx);
        theme::apply(app_core::UiTheme::Classic, cx);
        window.render_frame(cx);
        window.find("title-bar");
        window.find("workbench-tab-bar");
        window.find("workbench-status-bar");
        assert_eq!(
            window.find("workspace-heading").label(),
            Some("Recent capture")
        );
        assert_eq!(
            window.find("workbench-status-page").label(),
            Some("Recent capture")
        );
        window.find("workbench-status-message");
        window.find("workbench-status-folder");
        theme::apply(app_core::UiTheme::Default, cx);
        window.render_frame(cx);
        window.find("title-bar");
        assert!(window.try_find("workbench-tab-bar").is_none());
        assert_eq!(
            window.find("workspace-heading").label(),
            Some("Recent capture")
        );
    })
    .unwrap();
}

fn open_workspace(cx: &mut TestAppContext) -> WindowHandle<CaptureWorkspace> {
    cx.update(|cx| {
        cx.open_kit_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1060.), px(760.)),
                ))),
                ..app_window_options("screenshots")
            },
            |window, cx| {
                let view = cx.new(|cx| CaptureWorkspace::isolated_ui(window, cx));
                view.update(cx, |view, _| {
                    view.history_database = None;
                    view.output_directory = None;
                });
                let focus = view.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
                view
            },
        )
        .unwrap()
    })
}

#[gpui::test]
fn copy_file_path_requires_a_saved_capture_and_reports_worker_failure(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    handle
        .update(cx, |this, _, cx| {
            this.last_capture = Some(CapturedImage {
                width: 1,
                height: 1,
                pixels: Arc::new(image::RgbaImage::new(1, 1)),
                png_path: PathBuf::from("unsaved.png"),
                display_id: "fixture".into(),
                window_id: None,
                elapsed_ms: 0,
                source_format: app_core::PixelFormat::Rgba8UnormSrgb,
                display_geometry: None,
            });
            this.copy_last_file_path(cx);
            assert!(!this.copying);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("copy-last-file-path").disabled(), Some(true));
    })
    .unwrap();
    let path = PathBuf::from("C:/Saved captures/Grüße image.png");
    handle
        .update(cx, |this, _, cx| {
            this.last_saved = Some(path.clone());
            this.saving = true;
            this.copy_last_file_path(cx);
            assert!(!this.copying);
            this.saving = false;
            this.preview_active = true;
            this.copy_last_file_path(cx);
            assert!(!this.copying);
            this.preview_active = false;
            // Poison the state before native clipboard creation; the user's clipboard is untouched.
            let clipboard = this.clipboard.clone();
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _guard = clipboard.lock().unwrap();
                    panic!("synthetic clipboard state failure");
                }))
                .is_err()
            );
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_ne!(window.find("copy-last-file-path").disabled(), Some(true));
        window.click("copy-last-file-path", cx);
    })
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            assert!(this.copying);
            this.copy_last_file_path(cx);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert!(!this.copying);
            assert_eq!(this.last_saved.as_ref(), Some(&path));
            assert!(this.last_capture.is_some());
            assert_eq!(
                this.clipboard_status,
                "Could not copy file path: Clipboard state is unavailable"
            );
        })
        .unwrap();
}

#[cfg(target_os = "windows")]
#[gpui::test]
fn copy_file_rejects_missing_source_without_changing_clipboard_or_capture(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let missing =
        std::env::temp_dir().join(format!("missing-file-copy-{}.png", std::process::id()));
    assert!(!missing.exists());
    handle
        .update(cx, |this, _, cx| {
            this.copy_last_file(cx);
            assert!(!this.copying);
            this.last_saved = Some(missing.clone());
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("copy-last-file", cx);
    })
    .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.copying);
            assert!(
                this.clipboard_status
                    .starts_with("Could not copy file: Saved file is unavailable")
            );
            assert_eq!(this.last_saved.as_ref(), Some(&missing));
            assert!(this.last_capture.is_none());
            this.show_page(ControlPage::History, cx);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            this.history_entries = vec![app_core::HistoryEntry {
                id: 42,
                path: missing,
                width: 1,
                height: 1,
                created_at: "2026-11-12".into(),
            }];
            this.run_history_command(
                42,
                this.history_revision.wrapping_add(1),
                HistoryCommand::CopyFile,
                cx,
            );
            assert!(!this.copying);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("history-more-actions", cx);
        let bounds = window.find_by_label("Copy file").bounds();
        use gpui::test::TestEventExt;
        window.pointer_down(bounds.center(), gpui::MouseButton::Left, cx);
        window.pointer_up(bounds.center(), gpui::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert!(!this.copying);
            assert!(
                this.history_action_status
                    .starts_with("Could not copy file: Saved file is unavailable")
            );
            assert_eq!(this.history_entries.len(), 1);
        })
        .unwrap();
}

#[gpui::test]
fn custom_export_dimensions_validate_snapshot_and_release_input(cx: &mut TestAppContext) {
    for (index, axis, too_large, dimensions) in [
        (3, "Width", "6", (2, 2)),
        (4, "Height", "4", (4, 2)),
        (5, "Exact", "6", (2, 1)),
    ] {
        let handle = open_workspace(cx);
        let root =
            std::env::temp_dir().join(format!("screenshots-{axis}-save-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let pixels = Arc::new(image::RgbaImage::from_pixel(
            5,
            3,
            image::Rgba([12, 34, 56, 78]),
        ));
        handle
            .update(cx, |this, _, _| {
                this.output_directory = Some(root.clone());
                this.last_capture = Some(CapturedImage {
                    width: 5,
                    height: 3,
                    pixels: pixels.clone(),
                    png_path: root.join("unsaved.png"),
                    display_id: "fixture".into(),
                    window_id: None,
                    elapsed_ms: 0,
                    source_format: app_core::PixelFormat::Rgba8UnormSrgb,
                    display_geometry: None,
                });
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window
                .within("save-as-size")
                .click(if index == 3 { 4usize } else { 3usize }, cx);
            window.render_frame(cx);
            window.within("save-as-size").click(index, cx);
            window.render_frame(cx);
        })
        .unwrap();
        handle
            .update(cx, |this, window, cx| {
                let input = this.export_dimension_input.clone().unwrap();
                assert_eq!(
                    input.read(cx).value().as_str(),
                    if index == 4 { "480" } else { "640" }
                );
                if let Some(height) = this.export_height_input.clone() {
                    input.update(cx, |input, cx| input.set_value("2", window, cx));
                    for invalid in ["", "0", "4", "4294967295"] {
                        height.update(cx, |input, cx| input.set_value(invalid, window, cx));
                        this.save_capture_as(cx);
                        assert!(!this.saving);
                    }
                    height.update(cx, |input, cx| input.set_value("1", window, cx));
                }
                for invalid in ["", "0", too_large, "4294967295"] {
                    input.update(cx, |input, cx| input.set_value(invalid, window, cx));
                    this.save_capture_as(cx);
                    assert!(!this.saving);
                }
                input.update(cx, |input, cx| input.set_value("2", window, cx));
                this.save_capture_as(cx);
            })
            .unwrap();
        assert!(cx.did_prompt_for_new_path());
        handle
            .update(cx, |this, window, cx| {
                this.export_dimension_input
                    .as_ref()
                    .unwrap()
                    .update(cx, |input, cx| input.set_value("1", window, cx))
            })
            .unwrap();
        handle
            .update(cx, |this, window, cx| {
                if let Some(height) = &this.export_height_input {
                    height.update(cx, |input, cx| input.set_value("2", window, cx));
                }
            })
            .unwrap();
        cx.simulate_new_path_selection(|_| Some(root.join("custom.webp")));
        cx.run_until_parked();
        let output = image::open(root.join("custom.webp")).unwrap().into_rgba8();
        assert_eq!(output.dimensions(), dimensions);
        assert!(output.pixels().all(|pixel| pixel.0 == [12, 34, 56, 78]));
        handle
            .update(cx, |this, _, _| {
                assert!(!this.saving && this.last_saved.is_none());
                assert!(Arc::ptr_eq(
                    &this.last_capture.as_ref().unwrap().pixels,
                    &pixels
                ));
                assert!(this.save_status.contains(&if index == 5 {
                    "2 x 1 px".to_string()
                } else {
                    format!("{axis} 2 px")
                }));
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.press(&shortcut("3"), cx)
        })
        .unwrap();
        handle
            .update(cx, |this, _, _| {
                assert!(this.export_dimension_input.is_none() && this.export_height_input.is_none())
            })
            .unwrap();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_file(root.join("custom.webp")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}

#[gpui::test]
fn scaled_save_as_snapshots_size_and_keeps_finalized_source_and_history(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!("screenshots-scaled-save-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut pixels = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 100, 50, 255]));
    for y in 0..2 {
        for x in 0..2 {
            pixels.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
        }
    }
    let pixels = Arc::new(pixels);
    let database = root.join("history.sqlite");
    handle
        .update(cx, |this, _, _| {
            this.output_directory = Some(root.clone());
            this.history_database = Some(database.clone());
            this.last_capture = Some(CapturedImage {
                width: 4,
                height: 4,
                pixels: pixels.clone(),
                png_path: root.join("unsaved-source.png"),
                display_id: "fixture".into(),
                window_id: None,
                elapsed_ms: 0,
                source_format: app_core::PixelFormat::Rgba8UnormSrgb,
                display_geometry: None,
            });
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.within("save-as-size").click(1usize, cx);
        window.click("save-capture-as", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
    handle
        .update(cx, |this, _, _| {
            this.export_size = app_core::ExportSize::Quarter
        })
        .unwrap();
    cx.simulate_new_path_selection(|_| Some(root.join("small")));
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert!(!this.saving);
            assert!(Arc::ptr_eq(
                &this.last_capture.as_ref().unwrap().pixels,
                &pixels
            ));
            assert!(this.last_saved.is_none() && !database.exists());
            assert!(this.save_status.contains("Size 50%"));
            assert_eq!(this.clipboard_status, "No capture yet");
        })
        .unwrap();
    let output = image::open(root.join("small.png")).unwrap().into_rgba8();
    assert_eq!(output.dimensions(), (2, 2));
    assert_eq!(output.get_pixel(0, 0).0, [0, 0, 0, 255]);
    assert_eq!(output.get_pixel(1, 1).0, [200, 100, 50, 255]);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    std::fs::remove_file(root.join("small.png")).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[gpui::test]
fn filename_template_validation_failure_and_input_lifetime_are_bounded(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let blocker =
        std::env::temp_dir().join(format!("screenshots-name-blocker-{}", std::process::id()));
    std::fs::write(&blocker, b"not a folder").unwrap();
    handle
        .update(cx, |this, _, _| {
            this.output_directory = Some(blocker.clone())
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut(","), cx);
        window.render_frame(cx);
    })
    .unwrap();
    handle
        .update(cx, |this, window, cx| {
            let input = this.filename_template_input.clone().unwrap();
            input.update(cx, |input, cx| input.set_value("../bad", window, cx));
            this.save_filename_template(cx);
            assert!(!this.settings_saving && this.filename_template.is_empty());
            assert!(
                this.settings_status
                    .starts_with("Filename template unchanged:")
            );
            input.update(cx, |input, cx| input.set_value("Shot-{date}", window, cx));
            this.save_filename_template(cx);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert!(!this.settings_saving && this.filename_template.is_empty());
            assert!(
                this.settings_status
                    .starts_with("Filename template unchanged:")
            );
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut("1"), cx)
    })
    .unwrap();
    handle
        .update(cx, |this, _, _| {
            assert!(this.filename_template_input.is_none())
        })
        .unwrap();
    std::fs::remove_file(blocker).unwrap();
}

#[gpui::test]
fn automatic_saves_use_custom_names_and_track_the_published_last_path(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!("screenshots-name-output-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let database = root.join("history.sqlite");
    let pixels = Arc::new(image::RgbaImage::from_pixel(
        3,
        2,
        image::Rgba([12, 34, 56, 255]),
    ));
    let shot = CapturedImage {
        width: 3,
        height: 2,
        pixels: pixels.clone(),
        png_path: root.join("original.png"),
        display_id: "fixture".into(),
        window_id: None,
        elapsed_ms: 0,
        source_format: app_core::PixelFormat::Rgba8UnormSrgb,
        display_geometry: None,
    };
    handle
        .update(cx, |this, _, cx| {
            this.output_directory = Some(root.clone());
            this.history_database = Some(database.clone());
            this.filename_template = "Shot-{date}_{time}".into();
            this.last_capture = Some(shot.clone());
            this.output_captures(vec![shot.clone(), shot.clone()], false, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let saved = cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert!(!state.saving && state.last_capture.is_none());
        assert_eq!(state.clipboard_status, "No capture yet");
        assert_eq!(state.save_status, "Save: 2 PNG(s) saved");
        state.last_saved.clone().unwrap()
    });
    assert_eq!(saved.parent().unwrap(), root);
    assert!(
        saved
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("Shot-")
    );
    assert!(!shot.png_path.exists());
    assert_eq!(image::open(&saved).unwrap().into_rgba8(), *pixels);
    let files: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "png"))
        .collect();
    assert_eq!(files.len(), 2);
    assert_ne!(files[0], files[1]);
    let history = app_core::capture_history_page(&database, None).unwrap();
    assert_eq!(history.len(), 2);
    assert!(history.iter().any(|entry| entry.path == saved));
    assert!(history.iter().all(|entry| files.contains(&entry.path)));
    for entry in std::fs::read_dir(&root).unwrap() {
        std::fs::remove_file(entry.unwrap().path()).unwrap();
    }
    std::fs::remove_dir(root).unwrap();
}

#[gpui::test]
fn default_export_format_failure_retains_active_choice(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let blocker =
        std::env::temp_dir().join(format!("screenshots-format-blocker-{}", std::process::id()));
    std::fs::write(&blocker, b"not a folder").unwrap();
    handle
        .update(cx, |this, _, _| {
            this.output_directory = Some(blocker.clone());
            this.jpeg_quality = 73;
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut(","), cx);
        window.render_frame(cx);
        window.scroll(
            "choose-output-directory",
            gpui::ScrollDelta::Pixels(point(px(0.), px(-1400.))),
            cx,
        );
        window.render_frame(cx);
        window.within("save-as-format").click(1usize, cx);
    })
    .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert!(!this.settings_saving);
            assert_eq!(this.export_format, app_core::ImageExportFormat::Png);
            assert_eq!(this.jpeg_quality, 73);
            assert!(
                this.settings_status
                    .starts_with("Default export format unchanged:")
            );
        })
        .unwrap();
    std::fs::remove_file(blocker).unwrap();
}

#[gpui::test]
fn jpeg_quality_input_validates_preserves_failed_saves_and_releases_on_exit(
    cx: &mut TestAppContext,
) {
    let handle = open_workspace(cx);
    let blocker =
        std::env::temp_dir().join(format!("screenshots-jpeg-blocker-{}", std::process::id()));
    std::fs::write(&blocker, b"not a folder").unwrap();
    handle
        .update(cx, |this, _, _| {
            this.output_directory = Some(blocker.clone())
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut(","), cx);
        window.render_frame(cx);
        window.scroll(
            "choose-output-directory",
            gpui::ScrollDelta::Pixels(point(px(0.), px(-900.))),
            cx,
        );
        window.render_frame(cx);
        let _ = window.find_by_label("JPEG quality (1–100)");
    })
    .unwrap();
    handle
        .update(cx, |this, window, cx| {
            let input = this.jpeg_quality_input.clone().unwrap();
            input.update(cx, |input, cx| input.set_value("101", window, cx));
            this.save_jpeg_quality(cx);
            assert!(!this.settings_saving);
            assert!(this.settings_status.starts_with("JPEG quality unchanged:"));
            input.update(cx, |input, cx| input.set_value("73", window, cx));
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "choose-output-directory",
            gpui::ScrollDelta::Pixels(point(px(0.), px(-900.))),
            cx,
        );
        window.render_frame(cx);
        window.click("save-jpeg-quality", cx);
    })
    .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert_eq!(this.jpeg_quality, 90);
            assert!(!this.settings_saving);
            assert!(this.settings_status.starts_with("JPEG quality unchanged:"));
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut("1"), cx)
    })
    .unwrap();
    handle
        .update(cx, |this, _, _| assert!(this.jpeg_quality_input.is_none()))
        .unwrap();
    std::fs::remove_file(blocker).unwrap();
}

#[gpui::test]
fn save_as_image_formats_keep_current_pixels_and_png_library_unchanged(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root =
        std::env::temp_dir().join(format!("screenshots-save-formats-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let pixels = Arc::new(image::RgbaImage::from_pixel(
        3,
        2,
        image::Rgba([12, 34, 56, 255]),
    ));
    let source = root.join("unsaved-source.png");
    let database = root.join("history.sqlite");
    handle
        .update(cx, |this, _, _| {
            this.output_directory = Some(root.clone());
            this.jpeg_quality = 31;
            this.export_format = app_core::ImageExportFormat::WebP;
            this.history_database = Some(database.clone());
            this.last_capture = Some(CapturedImage {
                width: 3,
                height: 2,
                pixels: pixels.clone(),
                png_path: source.clone(),
                display_id: "fixture".into(),
                window_id: None,
                elapsed_ms: 0,
                source_format: app_core::PixelFormat::Rgba8UnormSrgb,
                display_geometry: None,
            });
        })
        .unwrap();
    for extension in ["jpg", "webp", "tiff", "jpg"] {
        handle
            .update(cx, |this, _, cx| this.save_capture_as(cx))
            .unwrap();
        assert!(cx.did_prompt_for_new_path());
        let path = root.join(format!("export.{extension}"));
        cx.simulate_new_path_selection(|_| Some(path.clone()));
        cx.run_until_parked();
        handle
            .update(cx, |this, _, _| {
                assert!(!this.saving);
                assert!(Arc::ptr_eq(
                    &this.last_capture.as_ref().unwrap().pixels,
                    &pixels
                ));
                assert!(this.last_saved.is_none());
                assert!(!source.exists() && !database.exists());
                assert_eq!(this.clipboard_status, "No capture yet");
                if extension == "tiff" {
                    assert!(this.save_status.starts_with("Image not saved:"));
                }
            })
            .unwrap();
    }
    assert!(root.join("export.jpg").exists() && root.join("export.webp").exists());
    let mut expected = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut expected, 31)
        .encode_image(&*pixels)
        .unwrap();
    assert_eq!(std::fs::read(root.join("export.jpg")).unwrap(), expected);
    assert!(!root.join("export.tiff").exists() && !root.join("export.png").exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
    handle
        .update(cx, |this, _, _| {
            assert!(this.save_status.contains("already exists"))
        })
        .unwrap();
    handle
        .update(cx, |this, _, cx| this.save_capture_as(cx))
        .unwrap();
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert!(!this.saving && this.last_capture.is_some())
        })
        .unwrap();
    handle
        .update(cx, |this, _, cx| this.save_capture_as(cx))
        .unwrap();
    // A later settings change cannot affect the already opened export dialog.
    handle
        .update(cx, |this, _, _| {
            this.export_format = app_core::ImageExportFormat::Jpeg
        })
        .unwrap();
    let bare = root.join("default-export");
    cx.simulate_new_path_selection(|_| Some(bare));
    cx.run_until_parked();
    assert!(root.join("default-export.webp").exists());
    assert!(!root.join("default-export.jpg").exists() && !root.join("default-export.png").exists());
    assert_eq!(
        image::open(root.join("default-export.webp"))
            .unwrap()
            .into_rgba8(),
        *pixels
    );
    std::fs::remove_file(root.join("default-export.webp")).unwrap();
    for extension in ["jpg", "webp"] {
        std::fs::remove_file(root.join(format!("export.{extension}"))).unwrap();
    }
    std::fs::remove_dir(root).unwrap();
}

fn exact_display() -> DisplayInfo {
    DisplayInfo {
        id: "exact-fixture".into(),
        name: "Fixture display".into(),
        x: -320,
        y: 0,
        width: 320,
        height: 200,
        scale_factor: 1.5,
        rotation: 0.,
        is_primary: true,
    }
}

#[test]
fn exact_region_parser_rejects_unbounded_noninteger_and_outside_values() {
    let values = |items: [&str; 4]| items.map(String::from);
    let display = exact_display();
    assert_eq!(
        workspace::exact_region_request(&display, &values(["10", "20", "30", "40"]))
            .unwrap()
            .area(),
        app_core::PixelRegion {
            x: 10,
            y: 20,
            width: 30,
            height: 40
        }
    );
    for invalid in [
        "",
        "-1",
        "+1",
        "1.5",
        " 1",
        "١",
        "4294967296",
        "00000000000",
    ] {
        assert!(
            workspace::exact_region_request(&display, &values([invalid, "0", "1", "1"])).is_err()
        );
    }
    for items in [
        ["0", "0", "0", "1"],
        ["0", "0", "1", "0"],
        ["319", "0", "2", "1"],
        ["0", "199", "1", "2"],
        ["4294967295", "0", "1", "1"],
    ] {
        assert!(workspace::exact_region_request(&display, &values(items)).is_err());
    }
}

#[gpui::test]
fn exact_region_fields_are_named_validate_and_release_with_the_page(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    handle
        .update(cx, |this, _, _| {
            this.output_directory = Some(std::env::temp_dir());
            this.xcap_displays = vec![exact_display()];
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut("3"), cx);
        window.render_frame(cx);
        let _ = window.find_by_label("Region width");
        window.click("exact-region-exact-fixture", cx);
    })
    .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert!(this.exact_region_status.starts_with("The region must"));
            assert!(this.exact_region_inputs.is_some());
            assert!(!this.capturing && this.last_capture.is_none() && this.last_saved.is_none());
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut("1"), cx)
    })
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            assert!(this.exact_region_inputs.is_none());
            assert!(this.exact_region_status.is_empty());
            this.capture_exact_region(exact_display(), cx);
            assert!(!this.capturing);
        })
        .unwrap();
}

#[gpui::test]
fn region_presets_load_without_capture_reject_stale_actions_and_preserve_failed_saves(
    cx: &mut TestAppContext,
) {
    use gpui::test::TestEventExt as _;
    let handle = open_workspace(cx);
    let blocker =
        std::env::temp_dir().join(format!("screenshots-preset-blocker-{}", std::process::id()));
    std::fs::write(&blocker, b"not a folder").unwrap();
    let preset = app_core::RegionPreset::new(
        "Small",
        app_core::PixelRegion {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        },
    )
    .unwrap();
    handle
        .update(cx, |this, _, _| {
            this.output_directory = Some(blocker.clone());
            this.xcap_displays = vec![exact_display()];
            this.region_presets = vec![preset.clone()];
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut("3"), cx);
        window.render_frame(cx);
        let bounds = window.find_by_label("Use region preset Small").bounds();
        window.pointer_down(bounds.center(), gpui::MouseButton::Left, cx);
        window.pointer_up(bounds.center(), gpui::MouseButton::Left, cx);
    })
    .unwrap();
    handle
        .update(cx, |this, window, cx| {
            let values: Vec<_> = this
                .exact_region_inputs
                .as_ref()
                .unwrap()
                .iter()
                .map(|input| input.read(cx).value().to_string())
                .collect();
            assert_eq!(values, ["10", "20", "30", "40"]);
            assert_eq!(
                this.region_preset_name
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .value()
                    .as_ref(),
                "Small"
            );
            assert!(!this.capturing && this.last_capture.is_none());
            this.region_presets[0].x = 11;
            this.use_region_preset(&preset, window, cx);
            this.remove_region_preset(&preset, cx);
            assert_eq!(this.region_presets[0].x, 11);
            assert!(!this.settings_saving);
            this.save_region_preset(exact_display(), cx);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert!(!this.settings_saving);
            assert_eq!(this.region_presets[0].x, 11);
            assert!(
                this.exact_region_status
                    .starts_with("Region presets unchanged:")
            );
            assert!(!this.capturing && this.last_saved.is_none());
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut("1"), cx)
    })
    .unwrap();
    handle
        .update(cx, |this, _, _| {
            assert!(this.region_preset_name.is_none() && this.exact_region_inputs.is_none())
        })
        .unwrap();
    std::fs::remove_file(blocker).unwrap();
}

#[gpui::test]
fn open_image_cancel_and_invalid_input_release_admission(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    handle
        .update(cx, |this, _, _| {
            this.output_directory = Some(std::env::temp_dir())
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("open-image", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    handle
        .update(cx, |this, _, cx| {
            this.open_image(cx);
            assert!(this.capturing);
        })
        .unwrap();
    #[cfg(target_os = "windows")]
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("ctrl-shift-v", cx);
    })
    .unwrap();
    cx.update(|cx| {
        assert_eq!(
            handle.read(cx).unwrap().capture_status,
            "Choosing an image…"
        )
    });
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        None
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert!(!state.capturing);
        assert_eq!(state.capture_status, "Image selection canceled.");
        assert!(
            state.editor.is_none() && state.last_capture.is_none() && state.last_saved.is_none()
        );
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut("o"), cx)
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("missing-image.png")]));
    cx.run_until_parked();
    cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert!(!state.capturing);
        assert!(state.capture_status.starts_with("Could not load image:"));
        assert!(
            state.editor.is_none() && state.last_capture.is_none() && state.last_saved.is_none()
        );
    });
    handle
        .update(cx, |this, _, cx| this.open_image(cx))
        .unwrap();
    handle
        .update(cx, |this, _, _| this.quitting = true)
        .unwrap();
    cx.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("must-not-load.png")]));
    cx.run_until_parked();
    cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert!(!state.capturing && state.editor.is_none());
        assert_eq!(state.capture_status, "Choosing an image…");
    });
}

#[gpui::test]
fn open_image_preserves_source_and_opens_without_output(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!("screenshots-open-image-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("original.png");
    image::RgbImage::from_pixel(3, 2, image::Rgb([12, 34, 56]))
        .save_with_format(&path, image::ImageFormat::Jpeg)
        .unwrap();
    let original = std::fs::read(&path).unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.output_directory = Some(root.clone());
            this.open_image(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|_| Some(vec![path.clone()]));
    cx.run_until_parked();
    cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert!(!state.capturing);
        assert!(state.editor.is_some(), "{}", state.capture_status);
        assert!(state.last_capture.is_none() && state.last_saved.is_none());
        let editor = state.editor.unwrap().read(cx).unwrap();
        assert_eq!(editor.source.pixels.dimensions(), (3, 2));
    });
    let editor = cx.update(|cx| handle.read(cx).unwrap().editor.unwrap());
    cx.update_window(editor.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("export-upload").is_some());
        assert_eq!(window.find("export-upload").disabled(), Some(true));
    })
    .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&root).unwrap();
}

#[gpui::test]
fn library_open_image_opens_webp_without_importing_or_writing_output(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root =
        std::env::temp_dir().join(format!("screenshots-library-open-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("original.webp");
    let pixels = image::RgbaImage::from_pixel(3, 2, image::Rgba([12, 34, 56, 78]));
    pixels.save(&path).unwrap();
    let original = std::fs::read(&path).unwrap();
    handle
        .update(cx, |this, _, _| this.output_directory = Some(root.clone()))
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut("2"), cx);
        window.render_frame(cx);
        window.click("library-open-image", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|options| {
        assert!(!options.multiple && options.files && !options.directories);
        Some(vec![path.clone()])
    });
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert_eq!(this.page, ControlPage::Captures);
            assert!(!this.history_visible && this.history_search_input.is_none());
            assert!(!this.capturing && this.last_capture.is_none() && this.last_saved.is_none());
            assert!(this.history_entries.is_empty());
            let editor = this.editor.unwrap().read(cx).unwrap();
            assert_eq!(&*editor.source.pixels, &pixels);
        })
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(root).unwrap();
}

fn drop_paths(handle: WindowHandle<CaptureWorkspace>, paths: &[PathBuf], cx: &mut TestAppContext) {
    use gpui::InputEvent as _;
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let position = point(px(600.), px(350.));
        for event in [
            gpui::FileDropEvent::Entered {
                position,
                paths: gpui::ExternalPaths(paths.iter().cloned().collect()),
            },
            gpui::FileDropEvent::Pending { position },
            gpui::FileDropEvent::Submit { position },
            gpui::FileDropEvent::Ended,
        ] {
            window.dispatch_event(event.to_platform_input(), cx);
        }
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn file_drop_validates_admission_and_opens_one_image_without_output(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!("screenshots-drop-image-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("original.webp");
    image::RgbaImage::from_pixel(3, 2, image::Rgba([12, 34, 56, 78]))
        .save(&path)
        .unwrap();
    let original = std::fs::read(&path).unwrap();
    handle
        .update(cx, |this, _, _| {
            this.output_directory = Some(root.clone());
            this.capturing = true;
            this.capture_status = "Existing capture".into();
        })
        .unwrap();
    drop_paths(handle, std::slice::from_ref(&path), cx);
    handle
        .update(cx, |this, _, _| {
            assert_eq!(this.capture_status, "Existing capture");
            assert!(this.editor.is_none());
            this.capturing = false;
        })
        .unwrap();
    drop_paths(handle, &[path.clone(), path.clone()], cx);
    handle
        .update(cx, |this, _, _| {
            assert!(this.capture_status.starts_with("Drop exactly one"));
            assert!(!this.capturing && this.editor.is_none());
        })
        .unwrap();
    drop_paths(handle, &[root.join("missing.png")], cx);
    handle
        .update(cx, |this, _, _| {
            assert!(this.capture_status.starts_with("Could not load image:"));
            assert!(!this.capturing && this.editor.is_none());
        })
        .unwrap();
    drop_paths(handle, std::slice::from_ref(&root), cx);
    handle
        .update(cx, |this, _, _| {
            assert!(this.capture_status.starts_with("Could not load image:"));
            assert!(!this.capturing && this.editor.is_none());
        })
        .unwrap();
    drop_paths(handle, std::slice::from_ref(&path), cx);
    cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert!(state.editor.is_some(), "{}", state.capture_status);
        assert!(!state.capturing && state.last_capture.is_none() && state.last_saved.is_none());
        let editor = state.editor.unwrap().read(cx).unwrap();
        assert_eq!(editor.source.pixels.dimensions(), (3, 2));
        assert_eq!(editor.source.pixels.get_pixel(0, 0).0, [12, 34, 56, 78]);
    });
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&root).unwrap();
}

#[gpui::test]
fn output_profile_failure_preserves_active_choice_and_rejects_multi_image_output(
    cx: &mut TestAppContext,
) {
    let handle = open_workspace(cx);
    let blocker = std::env::temp_dir().join(format!(
        "screenshots-profile-blocker-{}",
        std::process::id()
    ));
    std::fs::write(&blocker, b"not a directory").unwrap();
    cx.update(|cx| {
        handle
            .update(cx, |this, _, _| {
                this.output_directory = Some(blocker.clone());
            })
            .unwrap()
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut(","), cx);
        window.render_frame(cx);
        window.within("capture-output-profile").click(0usize, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = handle.read(cx).unwrap();
        assert!(!view.settings_saving);
        assert_eq!(view.output_profile, CaptureOutputProfile::SaveAndCopy);
        assert!(
            view.settings_status
                .starts_with("Capture output unchanged:")
        );
    });
    cx.update(|cx| {
        handle
            .update(cx, |this, _, cx| {
                this.output_profile = CaptureOutputProfile::CopyOnly;
                this.start_capture(
                    vec![
                        CaptureJob::Display("first".into()),
                        CaptureJob::Display("second".into()),
                    ],
                    cx,
                );
                assert!(!this.capturing);
                assert!(this.capture_status.starts_with("Choose one display"));
                assert!(this.last_capture.is_none());
                assert!(!this.copying && !this.saving);
            })
            .unwrap()
    });
    std::fs::remove_file(blocker).unwrap();
}

#[cfg(target_os = "windows")]
#[gpui::test]
fn repeat_without_a_region_shows_capture_guidance_without_output(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    handle
        .update(cx, |state, _, cx| {
            state.output_directory = Some(std::env::temp_dir());
            state.show_page(ControlPage::Settings, cx);
            state.repeat_last_region(cx);
            assert_eq!(state.page, ControlPage::Captures);
            assert!(state.capture_status.starts_with("Select a region"));
            assert!(state.last_region.is_none() && state.last_capture.is_none());
            assert!(!state.capturing && !state.copying && !state.saving);
        })
        .unwrap();
}

#[cfg(target_os = "windows")]
#[gpui::test]
fn active_window_countdown_can_cancel_without_starting_capture(cx: &mut TestAppContext) {
    for target in [DelayedCapture::ActiveWindow, DelayedCapture::ActiveDisplay] {
        let handle = open_workspace(cx);
        cx.update(|cx| {
            handle
                .update(cx, |this, _, cx| {
                    this.output_directory = Some(std::env::temp_dir());
                    this.capture_delay_seconds = 10;
                    this.start_delayed_capture(target, cx);
                    assert!(this.capture_delay.is_some());
                    assert!(this.capture_status.contains("in 10 s"));
                    assert!(this.capture_status.contains("Focus the target window"));
                    this.cancel_capture_delay(cx);
                })
                .unwrap()
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(11));
        cx.run_until_parked();
        cx.update(|cx| {
            let view = handle.read(cx).unwrap();
            assert!(view.capture_delay.is_none());
            assert!(!view.capturing && !view.copying && !view.saving);
            assert!(view.last_capture.is_none() && view.editor.is_none());
            assert_eq!(view.capture_status, "Delayed capture canceled");
        });
    }
}

#[gpui::test]
fn capture_delay_validates_applies_snapshots_and_releases_input(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let blocker =
        std::env::temp_dir().join(format!("screenshots-delay-blocker-{}", std::process::id()));
    std::fs::write(&blocker, b"not a folder").unwrap();
    handle
        .update(cx, |this, _, _| {
            this.output_directory = Some(blocker.clone())
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    handle
        .update(cx, |this, window, cx| {
            assert_eq!(this.capture_delay_seconds, 3);
            let input = this.capture_delay_input.clone().unwrap();
            for value in ["", "0", "61", "99"] {
                input.update(cx, |input, cx| input.set_value(value, window, cx));
                this.apply_capture_delay(cx);
                assert_eq!(this.capture_delay_seconds, 3);
                assert!(this.capture_delay.is_none());
            }
            input.update(cx, |input, cx| input.set_value("5", window, cx));
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("apply-capture-delay", cx);
    })
    .unwrap();
    handle
        .update(cx, |this, _, _| {
            assert!(this.settings_saving);
            assert_eq!(this.capture_delay_seconds, 3);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert!(!this.settings_saving);
            assert_eq!(this.capture_delay_seconds, 3);
            assert!(this.capture_status.starts_with("Capture delay unchanged:"));
            this.capture_delay_seconds = 5;
            this.output_directory = Some(std::env::temp_dir());
        })
        .unwrap();
    handle
        .update(cx, |this, window, cx| {
            this.start_delayed_region(cx);
            assert!(this.capture_status.contains("in 5 s"));
            this.capture_delay_input
                .as_ref()
                .unwrap()
                .update(cx, |input, cx| input.set_value("1", window, cx));
            this.apply_capture_delay(cx);
            assert_eq!(this.capture_delay_seconds, 5);
            // Changing a later preference cannot alter the duration captured by the running job.
            this.capture_delay_seconds = 1;
        })
        .unwrap();
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(this.capture_delay.is_some());
            assert!(this.capture_status.contains("in 4 seconds"));
            this.cancel_capture_delay(cx);
        })
        .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(60));
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(this.capture_delay.is_none());
            assert!(!this.capturing && this.last_capture.is_none());
            assert_eq!(this.capture_status, "Delayed capture canceled");
            this.show_page(ControlPage::Sources, cx);
            assert!(this.capture_delay_input.is_none());
        })
        .unwrap();
    std::fs::remove_file(blocker).unwrap();
}

#[cfg(target_os = "windows")]
#[gpui::test]
fn shortcut_fields_are_named_and_released_with_settings(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let window: AnyWindowHandle = handle.into();
    cx.update_window(window, |_, window, cx| {
        window.press(&shortcut(","), cx);
        window.render_frame(cx);
        for (index, expected) in app_core::CaptureHotkeys::default().0.iter().enumerate() {
            assert_eq!(
                window.find(("capture-hotkey", index)).value(),
                Some(expected.as_str())
            );
        }
        // Isolated UI tests own no native hotkey manager or settings destination.
        assert_eq!(window.find("save-hotkeys").disabled(), Some(true));
        window.press(&shortcut("1"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.update(|cx| assert!(handle.read(cx).unwrap().hotkey_inputs.is_none()));
}

#[gpui::test]
fn kit_root_preserves_native_window_content_and_navigation(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let window: AnyWindowHandle = handle.into();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("workspace-heading").label(),
            Some("Recent capture")
        );
        assert_eq!(window.find("new-capture").disabled(), Some(true));
        window.click("new-capture", cx);
        assert_eq!(
            window.find("workspace-heading").label(),
            Some("Recent capture")
        );
        window.press(&shortcut(","), cx);
        assert_eq!(window.find("workspace-heading").label(), Some("Settings"));
        window.press(&shortcut("3"), cx);
        assert_eq!(
            window.find("workspace-heading").label(),
            Some("Displays & windows")
        );
    })
    .unwrap();
    cx.update(|cx| {
        assert!(WindowHandle::<CaptureWorkspace>::from_handle(window, cx).is_some());
        assert!(WindowHandle::<RedactionEditor>::from_handle(window, cx).is_none());
        assert_eq!(handle.read(cx).unwrap().page, ControlPage::Sources);
    });
    cx.update(|cx| {
        let content = window
            .downcast::<gpui::base::Root>()
            .unwrap()
            .read(cx)
            .unwrap()
            .view()
            .clone()
            .downcast::<CaptureWorkspace>()
            .unwrap();
        content.update(cx, |_, cx| {
            assert!(WindowHandle::<CaptureWorkspace>::from_handle(window, cx).is_some());
        });
    });
}

#[gpui::test]
fn kit_search_keeps_bounds_and_releases_state_on_page_change(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let window: AnyWindowHandle = handle.into();
    cx.update_window(window, |_, window, cx| window.press(&shortcut("2"), cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("library-search", cx);
        window.input("Grüße 😀", cx);
        assert_eq!(window.find("library-search").value(), Some("Grüße 😀"));
        window.input(&"x".repeat(513), cx);
        assert!(window.find("library-search").value().unwrap().len() <= 512);
        window.press("escape", cx);
        assert_eq!(window.find("library-search").value(), Some(""));
        window.press(&shortcut("1"), cx);
        assert_eq!(
            window.find("workspace-heading").label(),
            Some("Recent capture")
        );
    })
    .unwrap();
    cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert!(state.history_search_input.is_none());
        assert!(state.history_search_subscription.is_none());
        assert!(state.history_query.is_empty());
    });
}

#[gpui::test]
fn stale_library_menu_cannot_mutate_a_new_page(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let window: AnyWindowHandle = handle.into();
    cx.update_window(window, |_, window, cx| window.press(&shortcut("2"), cx))
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |state, _, _| {
            state.history_status.clear();
            state.history_action_status.clear();
            state.history_entries = vec![app_core::HistoryEntry {
                id: 42,
                path: PathBuf::from("Synthetic.png"),
                width: 2,
                height: 3,
                created_at: "2026-11-11".into(),
            }];
        })
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("history-more-actions", cx);
        assert!(window.find_by_label("Remove from library").visible());
    })
    .unwrap();
    handle
        .update(cx, |state, _, _| {
            state.history_revision = state.history_revision.wrapping_add(1)
        })
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        let item = window.find_by_label("Remove from library");
        let bounds = item.bounds();
        // Popup items expose semantic labels rather than caller-assigned IDs.
        use gpui::test::TestEventExt;
        window.pointer_down(bounds.center(), gpui::MouseButton::Left, cx);
        window.pointer_up(bounds.center(), gpui::MouseButton::Left, cx);
    })
    .unwrap();
    cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert_eq!(state.history_entries.len(), 1);
        assert!(state.history_action_status.is_empty());
        assert!(!state.history_mutating);
    });
}

#[gpui::test]
fn library_decodes_visible_page_thumbnails_and_releases_them(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!(
        "screenshots-gallery-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let database = root.join("history.sqlite3");
    let mut paths = Vec::new();
    for index in 0..21 {
        let path = root.join(format!("shot-{index:02}.png"));
        image::RgbaImage::from_pixel(2, 3, image::Rgba([7, 20, 110, 255]))
            .save(&path)
            .unwrap();
        app_core::record_saved_capture(&database, &path).unwrap();
        paths.push(path);
    }
    std::fs::remove_file(paths.last().unwrap()).unwrap();
    handle
        .update(cx, |this, _, _| {
            this.history_database = Some(database);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.press(&shortcut("2"), cx))
        .unwrap();
    cx.run_until_parked();
    let newest = handle
        .update(cx, |this, _, cx| {
            assert!(!this.history_loading);
            assert!(this.history_thumb_loading.is_none());
            assert_eq!(this.history_entries.len(), 20);
            assert!(this.history_has_more);
            assert_eq!(this.history_thumbs.len(), 19);
            assert_eq!(this.history_thumb_failed.len(), 1);
            let missing = this.history_entries[0].id;
            assert!(this.history_thumb_failed.contains(&missing));
            assert!(!this.history_thumbs.contains_key(&missing));
            let cached = this.history_entries[1].id;
            let render = this.history_thumbs.get(&cached).cloned().unwrap();
            let bytes = render.as_bytes(0).unwrap();
            assert_eq!(bytes.len(), 2 * 3 * 4);
            assert_eq!(&bytes[..4], &[110, 20, 7, 255]);
            this.preview_history_image(cached, this.history_revision, cx);
            assert!(!this.capturing);
            assert!(
                this.history_preview
                    .as_ref()
                    .is_some_and(|(id, _)| *id == cached)
            );
            this.preview_history_image(cached, this.history_revision, cx);
            assert!(this.history_preview.is_none());
            assert!(this.history_thumbs.contains_key(&cached));
            this.refresh_history(Some(this.history_entries.last().unwrap().id), cx);
            cached
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert_eq!(this.history_entries.len(), 1);
            assert!(!this.history_has_more);
            assert_eq!(this.history_thumbs.len(), 1);
            assert!(this.history_thumb_failed.is_empty());
            assert!(!this.history_thumbs.contains_key(&newest));
            let older = this.history_entries[0].id;
            assert!(this.history_thumbs.contains_key(&older));
            this.show_page(ControlPage::Captures, cx);
            assert!(this.history_thumbs.is_empty());
            assert!(this.history_thumb_failed.is_empty());
            assert!(this.history_thumb_loading.is_none());
        })
        .unwrap();
    let extra = root.join("preview-miss.png");
    image::RgbaImage::from_pixel(3, 2, image::Rgba([11, 22, 33, 255]))
        .save(&extra)
        .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.history_visible = true;
            this.history_entries = vec![app_core::HistoryEntry {
                id: 99,
                path: extra.clone(),
                width: 3,
                height: 2,
                created_at: "2026-11-14".into(),
            }];
            this.preview_history_image(99, this.history_revision, cx);
            assert!(this.capturing);
            assert!(this.history_preview_cancel.is_some());
            this.preview_history_image(99, this.history_revision, cx);
            assert!(this.capturing);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.capturing);
            let (id, render) = this.history_preview.as_ref().unwrap();
            assert_eq!(*id, 99);
            let bytes = render.as_bytes(0).unwrap();
            assert_eq!(bytes.len(), 3 * 2 * 4);
            assert_eq!(&bytes[..4], &[33, 22, 11, 255]);
            assert!(this.history_thumbs.contains_key(&99));
            this.hide_history(cx);
            assert!(this.history_thumbs.is_empty());
            assert!(this.history_preview.is_none());
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn upload_setup_validates_without_network_and_discards_closed_form_results(
    cx: &mut TestAppContext,
) {
    let handle = open_workspace(cx);
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("setup-upload-destination", cx);
        window.render_frame(cx);
        assert_eq!(window.find("upload-setup-url").value(), Some(""));
    })
    .unwrap();
    handle
        .update(cx, |this, window, cx| {
            assert!(!this.upload_form_allow_http);
            let inputs = this.upload_form_inputs.clone().unwrap();
            inputs[0].update(cx, |input, cx| {
                input.set_value("http://127.0.0.1:12345/upload", window, cx)
            });
            this.use_upload_form(cx);
            assert!(this.upload_config_loading);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://old.invalid"
            );
            this.use_upload_form(cx);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.starts_with("Destination unchanged:"));
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://old.invalid"
            );
            this.upload_form_allow_http = true;
            this.upload_form_method = app_core::UploadMethod::Put;
            this.upload_form_json = true;
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "setup-upload-destination",
            gpui::ScrollDelta::Pixels(point(px(0.), px(-450.))),
            cx,
        );
        window.render_frame(cx);
        window.click("use-upload-setup", cx);
    })
    .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            let destination = this.upload_destination.as_ref().unwrap();
            assert_eq!(destination.origin(), "http://127.0.0.1:12345");
            assert_eq!(destination.request_method(), "PUT");
            assert!(!this.upload_form_open && this.upload_form_inputs.is_none());
            assert!(this.upload_cancel.is_none() && this.upload_generation == 0);
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            this.upload_form_open = true;
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    handle
        .update(cx, |this, window, cx| {
            this.upload_form_inputs.as_ref().unwrap()[0].update(cx, |input, cx| {
                input.set_value("https://new.invalid/upload", window, cx)
            });
            this.use_upload_form(cx);
            this.show_page(ControlPage::Sources, cx);
            assert!(this.upload_form_inputs.is_none() && !this.upload_form_open);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert!(!this.upload_config_loading);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "http://127.0.0.1:12345"
            );
            assert!(this.upload_cancel.is_none() && this.upload_generation == 0);
        })
        .unwrap();
}

#[cfg(target_os = "windows")]
#[gpui::test]
fn upload_credential_form_validates_without_writing_secrets(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "http://127.0.0.1:1/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    true,
                )
                .unwrap(),
            );
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("save-upload-credential", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("upload-credential-reference").value(),
            Some("main")
        );
        assert!(window.try_find("upload-setup-url").is_none());
    })
    .unwrap();
    handle
        .update(cx, |this, window, cx| {
            assert_eq!(
                this.upload_credential_auth,
                app_core::UploadAuthentication::Bearer
            );
            let inputs = this.upload_credential_inputs.clone().unwrap();
            inputs[2].update(cx, |input, cx| {
                input.set_value("synthetic-token_123.abc==", window, cx)
            });
            this.save_upload_credential(cx);
            assert!(this.upload_config_loading);
            assert!(
                this.upload_destination
                    .as_ref()
                    .unwrap()
                    .credential_reference()
                    .is_none()
            );
            this.save_upload_credential(cx);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.starts_with("Credential unchanged:"));
            assert!(!this.upload_status.contains("synthetic-token"));
            assert!(
                this.upload_destination
                    .as_ref()
                    .unwrap()
                    .credential_reference()
                    .is_none()
            );
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.upload_credential_open = true;
            this.upload_credential_inputs = None;
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    handle
        .update(cx, |this, window, cx| {
            this.upload_credential_inputs.as_ref().unwrap()[2]
                .update(cx, |input, cx| input.set_value("", window, cx));
            this.save_upload_credential(cx);
            this.show_page(ControlPage::Sources, cx);
            assert!(this.upload_credential_inputs.is_none() && !this.upload_credential_open);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert!(!this.upload_config_loading);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://old.invalid"
            );
            assert!(
                this.upload_destination
                    .as_ref()
                    .unwrap()
                    .credential_reference()
                    .is_none()
            );
            assert!(this.upload_cancel.is_none() && this.upload_generation == 0);
        })
        .unwrap();
}

#[gpui::test]
fn named_upload_destinations_switch_in_session_without_writing_catalog(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    handle
        .update(cx, |this, _, cx| {
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("save-named-upload-destination").is_none());
    })
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.upload_url = Some("https://old.invalid/share/kept".into());
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("save-named-upload-destination", cx);
        window.render_frame(cx);
        assert_eq!(window.find("upload-named-name").value(), Some(""));
        assert!(window.try_find("upload-setup-url").is_none());
        window.click("use-named-upload-save", cx);
    })
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            assert!(this.upload_config_loading);
            this.save_named_upload_destination(cx);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert!(
                this.upload_status
                    .starts_with("Named destinations unchanged:")
            );
            assert!(this.upload_named.is_empty());
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://old.invalid"
            );
            this.upload_named = vec![
                app_core::NamedUploadDestination::new(
                    "Host".into(),
                    app_core::UploadDestination::manual(
                        "https://named.invalid/upload".into(),
                        app_core::UploadMethod::Post,
                        "file".into(),
                        None,
                        false,
                    )
                    .unwrap(),
                )
                .unwrap(),
            ];
            this.use_named_upload_destination("Missing", cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://old.invalid"
            );
            assert_eq!(
                this.upload_url.as_deref(),
                Some("https://old.invalid/share/kept")
            );
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.use_named_upload_destination("Host", cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://old.invalid"
            );
            this.upload_cancel = None;
            this.use_named_upload_destination("Host", cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://named.invalid"
            );
            assert!(this.upload_url.is_none());
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            this.show_page(ControlPage::Sources, cx);
            assert!(!this.upload_named_open && this.upload_named_input.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn upload_task_defaults_apply_on_capture_without_uploading_or_persisting(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    handle
        .update(cx, |this, _, cx| {
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "setup-upload-destination",
            gpui::ScrollDelta::Pixels(point(px(0.), px(-2400.))),
            cx,
        );
        window.render_frame(cx);
        let _ = window.find("upload-task-defaults");
        assert!(window.try_find("upload-task-default-0").is_none());
    })
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.upload_url = Some("https://old.invalid/share/kept".into());
            this.upload_named = vec![
                app_core::NamedUploadDestination::new(
                    "Host".into(),
                    app_core::UploadDestination::manual(
                        "https://named.invalid/upload".into(),
                        app_core::UploadMethod::Post,
                        "file".into(),
                        None,
                        false,
                    )
                    .unwrap(),
                )
                .unwrap(),
            ];
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "setup-upload-destination",
            gpui::ScrollDelta::Pixels(point(px(0.), px(-2400.))),
            cx,
        );
        window.render_frame(cx);
        let _ = window.find("upload-task-defaults");
        assert!(window.try_find("use-named-upload-save").is_none());
    })
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_task_defaults[0] = "Host".into();
            this.upload_named_open = true;
            this.capture_primary(cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://named.invalid"
            );
            assert!(this.upload_url.is_none());
            assert!(this.upload_cancel.is_none() && this.upload_generation == 0);
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            assert!(!this.capturing);
            assert!(this.upload_named_open);
            this.upload_url = Some("https://named.invalid/share/kept".into());
            this.upload_task_defaults[0].clear();
            this.capture_primary(cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://named.invalid"
            );
            assert_eq!(
                this.upload_url.as_deref(),
                Some("https://named.invalid/share/kept")
            );
            this.upload_task_defaults[0] = "Missing".into();
            this.capture_primary(cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://named.invalid"
            );
            this.upload_task_defaults[0] = "Host".into();
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.capture_primary(cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://old.invalid"
            );
            this.upload_cancel = None;
            this.settings_saving = true;
            this.choose_upload_task_default(0, 0, cx);
            assert!(this.settings_saving);
            this.settings_saving = false;
            this.show_page(ControlPage::Sources, cx);
            this.capture_primary(cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://named.invalid"
            );
            assert!(this.upload_url.is_none());
            assert!(this.upload_cancel.is_none() && this.upload_generation == 0);
        })
        .unwrap();
}

#[gpui::test]
fn output_task_profiles_apply_on_capture_without_persisting_the_session_profile(
    cx: &mut TestAppContext,
) {
    let handle = open_workspace(cx);
    handle
        .update(cx, |this, _, cx| {
            this.show_page(ControlPage::Settings, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "choose-output-directory",
            gpui::ScrollDelta::Pixels(point(px(0.), px(-400.))),
            cx,
        );
        window.render_frame(cx);
        let _ = window.find("capture-output-task-profiles");
    })
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.output_profile = CaptureOutputProfile::SaveAndCopy;
            this.output_task_profiles[0] = Some(CaptureOutputProfile::CopyOnly);
            this.start_capture_task(
                0,
                vec![
                    CaptureJob::Display("first".into()),
                    CaptureJob::Display("second".into()),
                ],
                cx,
            );
            assert!(!this.capturing);
            assert!(this.capture_status.starts_with("Choose one display"));
            assert_eq!(this.output_profile, CaptureOutputProfile::SaveAndCopy);
            assert_eq!(
                this.output_task_profiles[0],
                Some(CaptureOutputProfile::CopyOnly)
            );
            this.output_profile = CaptureOutputProfile::CopyOnly;
            this.output_task_profiles[0] = Some(CaptureOutputProfile::SaveAndCopy);
            this.capture_status.clear();
            this.start_capture(
                vec![
                    CaptureJob::Display("first".into()),
                    CaptureJob::Display("second".into()),
                ],
                cx,
            );
            assert!(!this.capturing);
            assert!(this.capture_status.starts_with("Choose one display"));
            assert_eq!(this.output_profile, CaptureOutputProfile::CopyOnly);
            this.output_task_profiles[0] = None;
            this.capture_status.clear();
            this.start_capture_task(
                0,
                vec![
                    CaptureJob::Display("first".into()),
                    CaptureJob::Display("second".into()),
                ],
                cx,
            );
            assert!(!this.capturing);
            assert!(this.capture_status.starts_with("Choose one display"));
            this.settings_saving = true;
            this.choose_output_task_profile(0, 1, cx);
            assert!(this.settings_saving);
            assert!(this.output_task_profiles[0].is_none());
            this.settings_saving = false;
            this.show_page(ControlPage::Sources, cx);
            this.choose_output_task_profile(0, 1, cx);
            assert!(this.output_task_profiles[0].is_none());
            assert_eq!(this.output_profile, CaptureOutputProfile::CopyOnly);
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
        })
        .unwrap();
}

#[gpui::test]
fn upload_configuration_export_writes_a_chosen_file_without_importing(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!(
        "screenshots-upload-config-ui-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.output_directory = Some(root.clone());
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("export-upload-configuration", cx);
        window.click("import-upload-configuration", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
    handle
        .update(cx, |this, _, cx| {
            assert!(this.upload_config_loading);
            this.export_upload_configuration(cx);
            this.import_upload_configuration(cx);
        })
        .unwrap();
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.contains("Configuration export canceled"));
            assert!(this.upload_destination.is_none() && this.upload_named.is_empty());
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap()
                .with_authentication("main".into(), app_core::UploadAuthentication::Bearer)
                .unwrap(),
            );
            this.upload_named = vec![
                app_core::NamedUploadDestination::new(
                    "Host".into(),
                    app_core::UploadDestination::manual(
                        "https://named.invalid/upload".into(),
                        app_core::UploadMethod::Post,
                        "file".into(),
                        None,
                        false,
                    )
                    .unwrap(),
                )
                .unwrap(),
            ];
            this.upload_url = Some("https://old.invalid/share/kept".into());
            this.export_upload_configuration(cx);
        })
        .unwrap();
    cx.simulate_new_path_selection(|_| Some(root.join("backup")));
    cx.run_until_parked();
    let exported = std::fs::read_to_string(root.join("backup.json")).unwrap();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.contains("Configuration exported"));
            assert!(!exported.contains("secret"));
            assert!(
                exported.contains("old.invalid")
                    && exported.contains("Host")
                    && exported.contains("main")
            );
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://old.invalid"
            );
            assert_eq!(
                this.upload_url.as_deref(),
                Some("https://old.invalid/share/kept")
            );
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            this.import_upload_configuration(cx);
        })
        .unwrap();
    assert!(cx.did_prompt_for_paths());
    handle
        .update(cx, |this, _, cx| this.import_upload_configuration(cx))
        .unwrap();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        None
    });
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.contains("Configuration import canceled"));
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://old.invalid"
            );
            assert_eq!(this.upload_named[0].name(), "Host");
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.export_upload_configuration(cx);
            this.import_upload_configuration(cx);
            assert!(this.upload_cancel.is_some());
            this.show_page(ControlPage::Sources, cx);
            assert!(!this.upload_named_open);
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn sharex_import_applies_a_session_destination_without_writing_settings(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!("screenshots-sharex-ui-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let profile = root.join("host.sxcu");
    std::fs::write(
        &profile,
        r#"{
        "Name": "Host",
        "DestinationType": "ImageUploader",
        "RequestURL": "https://sharex.invalid/upload",
        "URL": "{json:url}"
    }"#,
    )
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.upload_url = Some("https://old.invalid/share/kept".into());
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("import-sharex-destination", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    handle
        .update(cx, |this, _, cx| {
            assert!(this.upload_config_loading);
            this.import_sharex_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        None
    });
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.contains("ShareX import canceled"));
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://old.invalid"
            );
            assert_eq!(
                this.upload_url.as_deref(),
                Some("https://old.invalid/share/kept")
            );
            this.import_sharex_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|_| Some(vec![profile.clone()]));
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://sharex.invalid"
            );
            assert!(this.upload_status.contains("ShareX Host"));
            assert!(this.upload_status.contains("Session only"));
            assert!(this.upload_url.is_none());
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            assert!(this.upload_named.is_empty());
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.import_sharex_destination(cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://sharex.invalid"
            );
            this.show_page(ControlPage::Sources, cx);
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn sharex_export_writes_a_chosen_file_without_secrets(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!(
        "screenshots-sharex-export-ui-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.output_directory = Some(root.clone());
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("export-sharex-destination").is_none());
    })
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://sharex.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    Some("/url".into()),
                    false,
                )
                .unwrap(),
            );
            this.upload_url = Some("https://sharex.invalid/share/kept".into());
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("export-sharex-destination", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
    handle
        .update(cx, |this, _, cx| {
            assert!(this.upload_config_loading);
            this.export_sharex_destination(cx);
        })
        .unwrap();
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.contains("ShareX export canceled"));
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://sharex.invalid"
            );
            assert_eq!(
                this.upload_url.as_deref(),
                Some("https://sharex.invalid/share/kept")
            );
            this.export_sharex_destination(cx);
        })
        .unwrap();
    cx.simulate_new_path_selection(|_| Some(root.join("host")));
    cx.run_until_parked();
    let exported = std::fs::read_to_string(root.join("host.sxcu")).unwrap();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.contains("ShareX profile exported"));
            assert!(this.upload_status.contains("secrets were not written"));
            assert!(!exported.contains("secret"));
            assert!(!exported.contains("credential"));
            assert!(exported.contains("sharex.invalid") && exported.contains("{json:url}"));
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://sharex.invalid"
            );
            assert_eq!(
                this.upload_url.as_deref(),
                Some("https://sharex.invalid/share/kept")
            );
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.export_sharex_destination(cx);
            assert!(this.upload_cancel.is_some());
            this.show_page(ControlPage::Sources, cx);
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn load_destination_applies_custom_headers_without_uploading_or_persisting(
    cx: &mut TestAppContext,
) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!("screenshots-headers-ui-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let profile = root.join("destination.json");
    std::fs::write(
        &profile,
        r#"{
            "url": "https://headers.invalid/upload",
            "field": "file",
            "json_pointer": "/url",
            "headers": { "X-Client": "screenshots" }
        }"#,
    )
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.upload_url = Some("https://old.invalid/share/kept".into());
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("load-upload-destination", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    handle
        .update(cx, |this, _, cx| {
            assert!(this.upload_config_loading);
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        None
    });
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.contains("canceled"));
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|_| Some(vec![profile.clone()]));
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            let destination = this.upload_destination.as_ref().unwrap();
            assert_eq!(destination.origin(), "https://headers.invalid");
            assert_eq!(
                destination
                    .request_headers()
                    .get("X-Client")
                    .map(String::as_str),
                Some("screenshots")
            );
            assert!(this.upload_status.contains("Session only"));
            assert!(this.upload_url.is_none());
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.load_upload_destination(cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://headers.invalid"
            );
            this.show_page(ControlPage::Sources, cx);
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn load_destination_applies_query_parameters_without_uploading_or_persisting(
    cx: &mut TestAppContext,
) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!("screenshots-query-ui-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let profile = root.join("destination.json");
    std::fs::write(
        &profile,
        r#"{
            "url": "https://query.invalid/upload",
            "field": "file",
            "json_pointer": "/url",
            "query": { "format": "json" }
        }"#,
    )
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.upload_url = Some("https://old.invalid/share/kept".into());
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("load-upload-destination", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    handle
        .update(cx, |this, _, cx| {
            assert!(this.upload_config_loading);
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        None
    });
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.contains("canceled"));
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|_| Some(vec![profile.clone()]));
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            let destination = this.upload_destination.as_ref().unwrap();
            assert_eq!(destination.origin(), "https://query.invalid");
            assert_eq!(
                destination
                    .request_query()
                    .get("format")
                    .map(String::as_str),
                Some("json")
            );
            assert!(this.upload_status.contains("Session only"));
            assert!(this.upload_url.is_none());
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.load_upload_destination(cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://query.invalid"
            );
            this.show_page(ControlPage::Sources, cx);
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn load_destination_applies_form_fields_without_uploading_or_persisting(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!("screenshots-form-ui-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let profile = root.join("destination.json");
    std::fs::write(
        &profile,
        r#"{
            "url": "https://form.invalid/upload",
            "field": "file",
            "json_pointer": "/url",
            "form": { "title": "screenshot" }
        }"#,
    )
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.upload_url = Some("https://old.invalid/share/kept".into());
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("load-upload-destination", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    handle
        .update(cx, |this, _, cx| {
            assert!(this.upload_config_loading);
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        None
    });
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.contains("canceled"));
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|_| Some(vec![profile.clone()]));
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            let destination = this.upload_destination.as_ref().unwrap();
            assert_eq!(destination.origin(), "https://form.invalid");
            assert_eq!(
                destination.request_form().get("title").map(String::as_str),
                Some("screenshot")
            );
            assert!(this.upload_status.contains("Session only"));
            assert!(this.upload_url.is_none());
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.load_upload_destination(cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://form.invalid"
            );
            this.show_page(ControlPage::Sources, cx);
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn load_destination_applies_json_body_without_uploading_or_persisting(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root =
        std::env::temp_dir().join(format!("screenshots-json-body-ui-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let profile = root.join("destination.json");
    std::fs::write(
        &profile,
        r#"{
            "url": "https://json.invalid/upload",
            "field": "file",
            "json_pointer": "/url",
            "json": { "image": "{base64}", "type": "file" }
        }"#,
    )
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.upload_url = Some("https://old.invalid/share/kept".into());
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("load-upload-destination", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    handle
        .update(cx, |this, _, cx| {
            assert!(this.upload_config_loading);
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        None
    });
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.contains("canceled"));
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|_| Some(vec![profile.clone()]));
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            let destination = this.upload_destination.as_ref().unwrap();
            assert_eq!(destination.origin(), "https://json.invalid");
            assert_eq!(
                destination.request_json().get("image").map(String::as_str),
                Some("{base64}")
            );
            assert_eq!(
                destination.request_json().get("type").map(String::as_str),
                Some("file")
            );
            assert!(this.upload_status.contains("Session only"));
            assert!(this.upload_url.is_none());
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.load_upload_destination(cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://json.invalid"
            );
            this.show_page(ControlPage::Sources, cx);
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn load_destination_applies_composed_result_link_without_uploading_or_persisting(
    cx: &mut TestAppContext,
) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!(
        "screenshots-composed-link-ui-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let profile = root.join("destination.json");
    std::fs::write(
        &profile,
        r#"{
            "url": "https://composed.invalid/upload",
            "field": "file",
            "json_pointer": "/id",
            "link_prefix": "https://cdn.composed.invalid/",
            "link_suffix": ".png"
        }"#,
    )
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.upload_url = Some("https://old.invalid/share/kept".into());
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("load-upload-destination", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    handle
        .update(cx, |this, _, cx| {
            assert!(this.upload_config_loading);
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        None
    });
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.contains("canceled"));
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|_| Some(vec![profile.clone()]));
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            let destination = this.upload_destination.as_ref().unwrap();
            assert_eq!(destination.origin(), "https://composed.invalid");
            assert_eq!(destination.link_prefix(), "https://cdn.composed.invalid/");
            assert_eq!(destination.link_suffix(), ".png");
            assert!(this.upload_status.contains("Session only"));
            assert!(this.upload_url.is_none());
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.load_upload_destination(cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://composed.invalid"
            );
            this.show_page(ControlPage::Sources, cx);
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn load_destination_applies_extra_result_urls_without_uploading_or_persisting(
    cx: &mut TestAppContext,
) {
    let handle = open_workspace(cx);
    let root =
        std::env::temp_dir().join(format!("screenshots-extra-links-ui-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let profile = root.join("destination.json");
    std::fs::write(
        &profile,
        r#"{
            "url": "https://extras.invalid/upload",
            "field": "file",
            "json_pointer": "/url",
            "thumbnail": { "json_pointer": "/thumbnail_url" },
            "deletion": { "json_pointer": "/delete" }
        }"#,
    )
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.upload_url = Some("https://old.invalid/share/kept".into());
            this.upload_thumbnail = Some("https://old.invalid/t.png".into());
            this.upload_deletion = Some("https://old.invalid/delete".into());
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("copy-upload-thumbnail").is_some());
        assert!(window.try_find("copy-upload-deletion").is_some());
        window.click("load-upload-destination", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    handle
        .update(cx, |this, _, cx| {
            assert!(this.upload_config_loading);
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        None
    });
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.contains("canceled"));
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|_| Some(vec![profile.clone()]));
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            let destination = this.upload_destination.as_ref().unwrap();
            assert_eq!(destination.origin(), "https://extras.invalid");
            assert_eq!(
                destination.thumbnail_template().as_deref(),
                Some("{json:thumbnail_url}")
            );
            assert_eq!(
                destination.deletion_template().as_deref(),
                Some("{json:delete}")
            );
            assert!(this.upload_status.contains("Session only"));
            assert!(this.upload_url.is_none());
            assert!(this.upload_thumbnail.is_none());
            assert!(this.upload_deletion.is_none());
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.load_upload_destination(cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://extras.invalid"
            );
            this.show_page(ControlPage::Sources, cx);
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn load_destination_applies_regex_result_without_uploading_or_persisting(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root =
        std::env::temp_dir().join(format!("screenshots-regex-link-ui-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let profile = root.join("destination.json");
    std::fs::write(
        &profile,
        r#"{
            "url": "https://regex.invalid/upload",
            "field": "file",
            "response": { "type": "regex", "pattern": "href=\"([^\"]+)\"", "group": "1" }
        }"#,
    )
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.upload_url = Some("https://old.invalid/share/kept".into());
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("load-upload-destination", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    handle
        .update(cx, |this, _, cx| {
            assert!(this.upload_config_loading);
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        None
    });
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.contains("canceled"));
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|_| Some(vec![profile.clone()]));
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            let destination = this.upload_destination.as_ref().unwrap();
            assert_eq!(destination.origin(), "https://regex.invalid");
            assert_eq!(destination.regex_pattern(), Some(r#"href="([^"]+)""#));
            assert_eq!(destination.regex_group(), Some("1"));
            assert!(this.upload_status.contains("Session only"));
            assert!(this.upload_url.is_none());
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.load_upload_destination(cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://regex.invalid"
            );
            this.show_page(ControlPage::Sources, cx);
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn load_destination_applies_xml_result_without_uploading_or_persisting(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!("screenshots-xml-link-ui-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let profile = root.join("destination.json");
    std::fs::write(
        &profile,
        r#"{
            "url": "https://xml.invalid/upload",
            "field": "file",
            "response": { "type": "xml", "path": "/files/file[1]/url" }
        }"#,
    )
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.upload_url = Some("https://old.invalid/share/kept".into());
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("load-upload-destination", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_paths());
    handle
        .update(cx, |this, _, cx| {
            assert!(this.upload_config_loading);
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        None
    });
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            assert!(this.upload_status.contains("canceled"));
            assert!(this.upload_destination.is_none());
            this.load_upload_destination(cx);
        })
        .unwrap();
    cx.simulate_path_prompt_response(|_| Some(vec![profile.clone()]));
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.upload_config_loading);
            let destination = this.upload_destination.as_ref().unwrap();
            assert_eq!(destination.origin(), "https://xml.invalid");
            assert_eq!(destination.xml_path(), Some("/files/file[1]/url"));
            assert!(this.upload_status.contains("Session only"));
            assert!(this.upload_url.is_none());
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.load_upload_destination(cx);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://xml.invalid"
            );
            this.show_page(ControlPage::Sources, cx);
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn upload_image_accepts_unsaved_capture_without_writing_library(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    handle
        .update(cx, |this, _, cx| {
            this.last_capture = Some(CapturedImage {
                width: 1,
                height: 1,
                pixels: Arc::new(image::RgbaImage::new(1, 1)),
                png_path: PathBuf::from("unsaved.png"),
                display_id: "fixture".into(),
                window_id: None,
                elapsed_ms: 0,
                source_format: app_core::PixelFormat::Rgba8UnormSrgb,
                display_geometry: None,
            });
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "setup-upload-destination",
            gpui::ScrollDelta::Pixels(point(px(0.), px(-450.))),
            cx,
        );
        window.render_frame(cx);
        assert_eq!(window.find("upload-saved-png").disabled(), Some(true));
        assert_eq!(window.find("strip-upload-metadata").checked(), Some(false));
        assert_ne!(window.find("strip-upload-metadata").disabled(), Some(true));
    })
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://memory.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "setup-upload-destination",
            gpui::ScrollDelta::Pixels(point(px(0.), px(-450.))),
            cx,
        );
        window.render_frame(cx);
        assert_ne!(window.find("upload-saved-png").disabled(), Some(true));
    })
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.saving = true;
            this.start_upload(cx);
            assert!(this.upload_cancel.is_none());
            this.saving = false;
            this.editor_closing = true;
            this.start_upload(cx);
            assert!(this.upload_cancel.is_none());
            this.editor_closing = false;
            this.settings_saving = true;
            this.persist_strip_metadata_on_upload(true, cx);
            assert!(!this.strip_metadata_on_upload);
            this.settings_saving = false;
            assert!(this.last_saved.is_none());
            assert!(this.last_capture.is_some());
            this.show_page(ControlPage::Sources, cx);
        })
        .unwrap();
}

#[gpui::test]
fn settings_backup_export_writes_a_chosen_file_without_importing(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!(
        "screenshots-settings-backup-ui-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.output_directory = Some(root.clone());
            this.jpeg_quality = 73;
            this.filename_template = "Shot-{date}".into();
            this.strip_metadata_on_upload = true;
            this.output_profile = CaptureOutputProfile::CopyOnly;
            this.output_task_profiles[0] = Some(CaptureOutputProfile::SaveOnly);
            this.capture_delay_seconds = 7;
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://kept.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.upload_url = Some("https://kept.invalid/share".into());
            this.show_page(ControlPage::Settings, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "choose-output-directory",
            gpui::ScrollDelta::Pixels(point(px(0.), px(-3200.))),
            cx,
        );
        window.render_frame(cx);
        window.click("export-settings", cx);
        window.click("import-settings", cx);
    })
    .unwrap();
    assert!(cx.did_prompt_for_new_path());
    handle
        .update(cx, |this, _, cx| {
            assert!(this.settings_saving);
            this.export_settings(cx);
            this.import_settings(cx);
        })
        .unwrap();
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.settings_saving);
            assert!(this.settings_status.contains("Export canceled"));
            assert_eq!(this.jpeg_quality, 73);
            assert_eq!(this.output_profile, CaptureOutputProfile::CopyOnly);
            this.export_settings(cx);
        })
        .unwrap();
    cx.simulate_new_path_selection(|_| Some(root.join("backup")));
    cx.run_until_parked();
    let exported = std::fs::read_to_string(root.join("backup.json")).unwrap();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.settings_saving);
            assert!(this.settings_status.contains("Settings exported"));
            assert!(!exported.contains("secret"));
            assert!(!exported.contains("kept.invalid"));
            assert!(exported.contains("Shot-{date}") && exported.contains("copy_only"));
            assert!(exported.contains("save_only"));
            assert!(exported.contains("\"strip_metadata_on_upload\": true"));
            assert_eq!(this.jpeg_quality, 73);
            assert_eq!(
                this.upload_destination.as_ref().unwrap().origin(),
                "https://kept.invalid"
            );
            assert_eq!(
                this.upload_url.as_deref(),
                Some("https://kept.invalid/share")
            );
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            this.import_settings(cx);
        })
        .unwrap();
    assert!(cx.did_prompt_for_paths());
    handle
        .update(cx, |this, _, cx| this.import_settings(cx))
        .unwrap();
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        None
    });
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.settings_saving);
            assert!(this.settings_status.contains("Import canceled"));
            assert_eq!(this.jpeg_quality, 73);
            assert_eq!(this.output_profile, CaptureOutputProfile::CopyOnly);
            this.settings_saving = true;
            this.export_settings(cx);
            this.import_settings(cx);
            assert!(this.settings_saving);
            this.settings_saving = false;
            this.show_page(ControlPage::Sources, cx);
        })
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "windows")]
#[gpui::test]
fn upload_remove_credential_stays_idle_without_an_attached_reference(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    handle
        .update(cx, |this, _, cx| {
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("remove-upload-credential").is_none());
    })
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.upload_url = Some("https://old.invalid/share/kept".into());
            this.remove_upload_credential(cx);
            assert!(!this.upload_config_loading);
            assert_eq!(
                this.upload_destination
                    .as_ref()
                    .unwrap()
                    .credential_reference(),
                None
            );
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("remove-upload-credential").is_none());
    })
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_destination = Some(
                this.upload_destination
                    .take()
                    .unwrap()
                    .with_authentication("main".into(), app_core::UploadAuthentication::Bearer)
                    .unwrap(),
            );
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.remove_upload_credential(cx);
            assert!(!this.upload_config_loading);
            assert_eq!(
                this.upload_destination
                    .as_ref()
                    .unwrap()
                    .credential_reference(),
                Some("main")
            );
            this.upload_cancel = None;
            this.upload_config_loading = true;
            this.remove_upload_credential(cx);
            assert_eq!(
                this.upload_destination
                    .as_ref()
                    .unwrap()
                    .credential_reference(),
                Some("main")
            );
            this.upload_config_loading = false;
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "setup-upload-destination",
            gpui::ScrollDelta::Pixels(point(px(0.), px(-450.))),
            cx,
        );
        window.render_frame(cx);
        assert_ne!(
            window.find("remove-upload-credential").disabled(),
            Some(true)
        );
    })
    .unwrap();
    handle
        .update(cx, |this, _, _| {
            assert_eq!(
                this.upload_url.as_deref(),
                Some("https://old.invalid/share/kept")
            );
            assert_eq!(
                this.upload_destination
                    .as_ref()
                    .unwrap()
                    .credential_reference(),
                Some("main")
            );
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            assert!(!this.upload_config_loading);
        })
        .unwrap();
}

#[gpui::test]
fn upload_destination_probe_stays_idle_without_a_destination_or_saved_image(
    cx: &mut TestAppContext,
) {
    let handle = open_workspace(cx);
    handle
        .update(cx, |this, _, cx| {
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("test-upload-destination").is_none());
        assert!(window.try_find("cancel-upload").is_none());
    })
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_url = Some("https://old.invalid/share/kept".into());
            this.start_destination_probe(cx);
            assert!(
                this.upload_cancel.is_none() && this.upload_generation == 0 && !this.upload_probe
            );
            assert_eq!(
                this.upload_url.as_deref(),
                Some("https://old.invalid/share/kept")
            );
            this.upload_destination = Some(
                app_core::UploadDestination::manual(
                    "https://old.invalid/upload".into(),
                    app_core::UploadMethod::Post,
                    "file".into(),
                    None,
                    false,
                )
                .unwrap(),
            );
            this.saving = true;
            this.start_destination_probe(cx);
            assert!(
                this.upload_cancel.is_none() && this.upload_generation == 0 && !this.upload_probe
            );
            this.saving = false;
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.upload_generation = 4;
            this.start_destination_probe(cx);
            assert_eq!(this.upload_generation, 4);
            assert!(!this.upload_probe);
            this.upload_cancel = None;
            this.upload_generation = 0;
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "setup-upload-destination",
            gpui::ScrollDelta::Pixels(point(px(0.), px(-450.))),
            cx,
        );
        window.render_frame(cx);
        assert_ne!(
            window.find("test-upload-destination").disabled(),
            Some(true)
        );
        assert!(window.try_find("cancel-upload").is_none());
        assert_eq!(window.find("upload-saved-png").disabled(), Some(true));
    })
    .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.upload_probe = true;
            this.upload_cancel = Some(app_core::UploadCancellation::default());
            this.show_page(ControlPage::Uploads, cx);
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("test-upload-destination").disabled(),
            Some(true)
        );
        assert_ne!(window.find("cancel-upload").disabled(), Some(true));
        window.click("cancel-upload", cx);
    })
    .unwrap();
    handle
        .update(cx, |this, _, _| {
            assert!(this.upload_cancel_requested);
            assert!(this.upload_status.starts_with("Canceling test"));
            assert_eq!(
                this.upload_url.as_deref(),
                Some("https://old.invalid/share/kept")
            );
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            assert!(this.upload_history_entries.is_empty());
            this.upload_cancel = None;
            this.upload_probe = false;
            this.upload_generation = 0;
            this.upload_cancel_requested = false;
        })
        .unwrap();
}

#[gpui::test]
fn library_copy_path_routes_menu_and_discards_stale_status(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut("2"), cx)
    })
    .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            this.history_entries = vec![app_core::HistoryEntry {
                id: 42,
                path: PathBuf::from("C:/Saved captures/Grüße.png"),
                width: 2,
                height: 3,
                created_at: "2026-11-12".into(),
            }];
            let revision = this.history_revision;
            this.run_history_command(42, revision.wrapping_add(1), HistoryCommand::CopyPath, cx);
            this.run_history_command(43, revision, HistoryCommand::CopyPath, cx);
            this.history_loading = true;
            this.run_history_command(42, revision, HistoryCommand::CopyPath, cx);
            this.history_loading = false;
            assert!(!this.copying);
            let clipboard = this.clipboard.clone();
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _guard = clipboard.lock().unwrap();
                    panic!("synthetic clipboard state failure");
                }))
                .is_err()
            );
        })
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("history-more-actions", cx);
        let item = window.find_by_label("Copy file path");
        assert!(item.visible());
        use gpui::test::TestEventExt;
        let bounds = item.bounds();
        window.pointer_down(bounds.center(), gpui::MouseButton::Left, cx);
        window.pointer_up(bounds.center(), gpui::MouseButton::Left, cx);
    })
    .unwrap();
    handle
        .update(cx, |this, _, _| {
            assert!(this.copying);
            assert_eq!(this.history_action_status, "Copying file path…");
            this.history_revision = this.history_revision.wrapping_add(1);
            this.history_action_status = "New page status".into();
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.copying);
            assert_eq!(this.history_action_status, "New page status");
            assert!(this.last_saved.is_none() && this.last_capture.is_none());
            this.run_history_command(42, this.history_revision, HistoryCommand::CopyPath, cx);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, _| {
            assert!(!this.copying);
            assert_eq!(
                this.history_action_status,
                "Could not copy file path: Clipboard state is unavailable"
            );
            assert_eq!(this.history_entries.len(), 1);
        })
        .unwrap();
}

#[gpui::test]
fn quit_waits_for_work_inside_a_kit_root(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    handle
        .update(cx, |state, _, _| state.saving = true)
        .unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("workspace-footer-2", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert!(state.quitting);
        assert!(state.saving);
    });
}

#[gpui::test]
fn escape_releases_an_overlay_inside_its_kit_root(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    cx.update(|cx| cx.bind_keys([KeyBinding::new("escape", CloseOverlay, Some("Overlay"))]));
    handle
        .update(cx, |state, _, cx| {
            let source = CapturedImage {
                display_id: "fixture".into(),
                window_id: None,
                width: 4,
                height: 3,
                pixels: Arc::new(image::RgbaImage::new(4, 3)),
                png_path: PathBuf::from("unused-source.png"),
                elapsed_ms: 0,
                source_format: app_core::PixelFormat::Rgba8UnormSrgb,
                display_geometry: Some(app_core::DisplayGeometry {
                    x: -4,
                    y: 0,
                    width: 4,
                    height: 3,
                    scale_factor: 1.25,
                    rotation: 0.0,
                }),
            };
            state.set_last_region(Some(
                app_core::RememberedRegion::from_snapshot(
                    &source,
                    app_core::PixelRegion {
                        x: 1,
                        y: 1,
                        width: 2,
                        height: 2,
                    },
                )
                .unwrap(),
            ));
            state.show_overlays(OverlayMode::Probe, &[], &HashMap::new(), cx);
        })
        .unwrap();
    let overlay = cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert_eq!(state.live_overlay_count(cx), 1);
        state.overlays[0]
    });
    cx.update_window(overlay.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert_eq!(state.live_overlay_count(cx), 0);
        assert!(state.overlays.is_empty());
        assert!(state.last_saved.is_none());
        assert!(state.last_capture.is_none());
        assert_eq!(
            state.last_region.as_ref().unwrap().area(),
            app_core::PixelRegion {
                x: 1,
                y: 1,
                width: 2,
                height: 2
            }
        );
    });
}

#[gpui::test]
fn library_checkbox_keeps_controlled_selection_and_disabled_state(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let window: AnyWindowHandle = handle.into();
    cx.update_window(window, |_, window, cx| window.press(&shortcut("2"), cx))
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |state, _, _| {
            state.history_entries = vec![app_core::HistoryEntry {
                id: 42,
                path: PathBuf::from("Synthetic.png"),
                width: 2,
                height: 3,
                created_at: "2026-11-11".into(),
            }];
        })
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("select-history-entry", cx);
        assert_eq!(window.find("select-history-entry").checked(), Some(true));
    })
    .unwrap();
    cx.update(|cx| assert_eq!(handle.read(cx).unwrap().history_selected, vec![42]));
    handle
        .update(cx, |state, _, _| state.history_mutating = true)
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("select-history-entry").disabled(), Some(true));
        window.click("select-history-entry", cx);
    })
    .unwrap();
    cx.update(|cx| assert_eq!(handle.read(cx).unwrap().history_selected, vec![42]));
}
