use super::*;
use gpui::test::{TestQueryExt, TestWindowExt};
use gpui::{AppContext, TestAppContext};

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
        for appearance in [app_core::UiTheme::Oldschool, app_core::UiTheme::Default] {
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

fn open_workspace(cx: &mut TestAppContext) -> WindowHandle<CaptureWorkspace> {
    cx.update(|cx| {
        cx.open_kit_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1060.), px(760.)),
                ))),
                ..Default::default()
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
fn filename_template_validation_failure_and_input_lifetime_are_bounded(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let blocker = std::env::temp_dir().join(format!("screenshots-name-blocker-{}", std::process::id()));
    std::fs::write(&blocker, b"not a folder").unwrap();
    handle.update(cx, |this, _, _| this.output_directory = Some(blocker.clone())).unwrap();
    cx.update_window(handle.into(), |_, window, cx| { window.press(&shortcut(","), cx); window.render_frame(cx); }).unwrap();
    handle.update(cx, |this, window, cx| {
        let input = this.filename_template_input.clone().unwrap();
        input.update(cx, |input, cx| input.set_value("../bad", window, cx));
        this.save_filename_template(cx);
        assert!(!this.settings_saving && this.filename_template.is_empty());
        assert!(this.settings_status.starts_with("Filename template unchanged:"));
        input.update(cx, |input, cx| input.set_value("Shot-{date}", window, cx));
        this.save_filename_template(cx);
    }).unwrap();
    cx.run_until_parked();
    handle.update(cx, |this, _, _| {
        assert!(!this.settings_saving && this.filename_template.is_empty());
        assert!(this.settings_status.starts_with("Filename template unchanged:"));
    }).unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.press(&shortcut("1"), cx)).unwrap();
    handle.update(cx, |this, _, _| assert!(this.filename_template_input.is_none())).unwrap();
    std::fs::remove_file(blocker).unwrap();
}

#[gpui::test]
fn automatic_saves_use_custom_names_and_track_the_published_last_path(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!("screenshots-name-output-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let database = root.join("history.sqlite");
    let pixels = Arc::new(image::RgbaImage::from_pixel(3, 2, image::Rgba([12, 34, 56, 255])));
    let shot = CapturedImage { width: 3, height: 2, pixels: pixels.clone(), png_path: root.join("original.png"),
        display_id: "fixture".into(), window_id: None, elapsed_ms: 0,
        source_format: app_core::PixelFormat::Rgba8UnormSrgb, display_geometry: None };
    handle.update(cx, |this, _, cx| {
        this.output_directory = Some(root.clone());
        this.history_database = Some(database.clone());
        this.filename_template = "Shot-{date}_{time}".into();
        this.last_capture = Some(shot.clone());
        this.output_captures(vec![shot.clone(), shot.clone()], false, cx);
    }).unwrap();
    cx.run_until_parked();
    let saved = cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert!(!state.saving && state.last_capture.is_none());
        assert_eq!(state.clipboard_status, "No capture yet");
        assert_eq!(state.save_status, "Save: 2 PNG(s) saved");
        state.last_saved.clone().unwrap()
    });
    assert_eq!(saved.parent().unwrap(), root);
    assert!(saved.file_name().unwrap().to_string_lossy().starts_with("Shot-"));
    assert!(!shot.png_path.exists());
    assert_eq!(image::open(&saved).unwrap().into_rgba8(), *pixels);
    let files: Vec<_> = std::fs::read_dir(&root).unwrap().map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "png")).collect();
    assert_eq!(files.len(), 2);
    assert_ne!(files[0], files[1]);
    let history = app_core::capture_history_page(&database, None).unwrap();
    assert_eq!(history.len(), 2);
    assert!(history.iter().any(|entry| entry.path == saved));
    assert!(history.iter().all(|entry| files.contains(&entry.path)));
    for entry in std::fs::read_dir(&root).unwrap() { std::fs::remove_file(entry.unwrap().path()).unwrap(); }
    std::fs::remove_dir(root).unwrap();
}

#[gpui::test]
fn default_export_format_failure_retains_active_choice(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let blocker = std::env::temp_dir().join(format!("screenshots-format-blocker-{}", std::process::id()));
    std::fs::write(&blocker, b"not a folder").unwrap();
    handle.update(cx, |this, _, _| {
        this.output_directory = Some(blocker.clone());
        this.jpeg_quality = 73;
    }).unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut(","), cx);
        window.render_frame(cx);
        window.scroll("choose-output-directory", gpui::ScrollDelta::Pixels(point(px(0.), px(-650.))), cx);
        window.render_frame(cx);
        window.within("save-as-format").click(1usize, cx);
    }).unwrap();
    cx.run_until_parked();
    handle.update(cx, |this, _, _| {
        assert!(!this.settings_saving);
        assert_eq!(this.export_format, app_core::ImageExportFormat::Png);
        assert_eq!(this.jpeg_quality, 73);
        assert!(this.settings_status.starts_with("Default export format unchanged:"));
    }).unwrap();
    std::fs::remove_file(blocker).unwrap();
}

#[gpui::test]
fn jpeg_quality_input_validates_preserves_failed_saves_and_releases_on_exit(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let blocker = std::env::temp_dir().join(format!("screenshots-jpeg-blocker-{}", std::process::id()));
    std::fs::write(&blocker, b"not a folder").unwrap();
    handle.update(cx, |this, _, _| this.output_directory = Some(blocker.clone())).unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut(","), cx);
        window.render_frame(cx);
        let _ = window.find_by_label("JPEG quality (1–100)");
    }).unwrap();
    handle.update(cx, |this, window, cx| {
        let input = this.jpeg_quality_input.clone().unwrap();
        input.update(cx, |input, cx| input.set_value("101", window, cx));
        this.save_jpeg_quality(cx);
        assert!(!this.settings_saving);
        assert!(this.settings_status.starts_with("JPEG quality unchanged:"));
        input.update(cx, |input, cx| input.set_value("73", window, cx));
    }).unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.scroll("choose-output-directory", gpui::ScrollDelta::Pixels(point(px(0.), px(-450.))), cx);
        window.render_frame(cx);
        window.click("save-jpeg-quality", cx);
    }).unwrap();
    cx.run_until_parked();
    handle.update(cx, |this, _, _| {
        assert_eq!(this.jpeg_quality, 90);
        assert!(!this.settings_saving);
        assert!(this.settings_status.starts_with("JPEG quality unchanged:"));
    }).unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.press(&shortcut("1"), cx)).unwrap();
    handle.update(cx, |this, _, _| assert!(this.jpeg_quality_input.is_none())).unwrap();
    std::fs::remove_file(blocker).unwrap();
}

#[gpui::test]
fn save_as_image_formats_keep_current_pixels_and_png_library_unchanged(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!("screenshots-save-formats-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let pixels = Arc::new(image::RgbaImage::from_pixel(3, 2, image::Rgba([12, 34, 56, 255])));
    let source = root.join("unsaved-source.png");
    let database = root.join("history.sqlite");
    handle.update(cx, |this, _, _| {
        this.output_directory = Some(root.clone());
        this.jpeg_quality = 31;
        this.export_format = app_core::ImageExportFormat::WebP;
        this.history_database = Some(database.clone());
        this.last_capture = Some(CapturedImage { width: 3, height: 2, pixels: pixels.clone(),
            png_path: source.clone(), display_id: "fixture".into(), window_id: None, elapsed_ms: 0,
            source_format: app_core::PixelFormat::Rgba8UnormSrgb, display_geometry: None });
    }).unwrap();
    for extension in ["jpg", "webp", "tiff", "jpg"] {
        handle.update(cx, |this, _, cx| this.save_capture_as(cx)).unwrap();
        assert!(cx.did_prompt_for_new_path());
        let path = root.join(format!("export.{extension}"));
        cx.simulate_new_path_selection(|_| Some(path.clone()));
        cx.run_until_parked();
        handle.update(cx, |this, _, _| {
            assert!(!this.saving);
            assert!(Arc::ptr_eq(&this.last_capture.as_ref().unwrap().pixels, &pixels));
            assert!(this.last_saved.is_none());
            assert!(!source.exists() && !database.exists());
            assert_eq!(this.clipboard_status, "No capture yet");
            if extension == "tiff" { assert!(this.save_status.starts_with("Image not saved:")); }
        }).unwrap();
    }
    assert!(root.join("export.jpg").exists() && root.join("export.webp").exists());
    let mut expected = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut expected, 31).encode_image(&*pixels).unwrap();
    assert_eq!(std::fs::read(root.join("export.jpg")).unwrap(), expected);
    assert!(!root.join("export.tiff").exists() && !root.join("export.png").exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
    handle.update(cx, |this, _, _| assert!(this.save_status.contains("already exists"))).unwrap();
    handle.update(cx, |this, _, cx| this.save_capture_as(cx)).unwrap();
    cx.simulate_new_path_selection(|_| None);
    cx.run_until_parked();
    handle.update(cx, |this, _, _| assert!(!this.saving && this.last_capture.is_some())).unwrap();
    handle.update(cx, |this, _, cx| this.save_capture_as(cx)).unwrap();
    // A later settings change cannot affect the already opened export dialog.
    handle.update(cx, |this, _, _| this.export_format = app_core::ImageExportFormat::Jpeg).unwrap();
    let bare = root.join("default-export");
    cx.simulate_new_path_selection(|_| Some(bare));
    cx.run_until_parked();
    assert!(root.join("default-export.webp").exists());
    assert!(!root.join("default-export.jpg").exists() && !root.join("default-export.png").exists());
    assert_eq!(image::open(root.join("default-export.webp")).unwrap().into_rgba8(), *pixels);
    std::fs::remove_file(root.join("default-export.webp")).unwrap();
    for extension in ["jpg", "webp"] { std::fs::remove_file(root.join(format!("export.{extension}"))).unwrap(); }
    std::fs::remove_dir(root).unwrap();
}

fn exact_display() -> DisplayInfo {
    DisplayInfo { id: "exact-fixture".into(), name: "Fixture display".into(), x: -320, y: 0,
        width: 320, height: 200, scale_factor: 1.5, rotation: 0., is_primary: true }
}

#[test]
fn exact_region_parser_rejects_unbounded_noninteger_and_outside_values() {
    let values = |items: [&str; 4]| items.map(String::from);
    let display = exact_display();
    assert_eq!(workspace::exact_region_request(&display, &values(["10", "20", "30", "40"])).unwrap().area(),
        app_core::PixelRegion { x: 10, y: 20, width: 30, height: 40 });
    for invalid in ["", "-1", "+1", "1.5", " 1", "١", "4294967296", "00000000000"] {
        assert!(workspace::exact_region_request(&display, &values([invalid, "0", "1", "1"])).is_err());
    }
    for items in [["0", "0", "0", "1"], ["0", "0", "1", "0"],
        ["319", "0", "2", "1"], ["0", "199", "1", "2"], ["4294967295", "0", "1", "1"]] {
        assert!(workspace::exact_region_request(&display, &values(items)).is_err());
    }
}

#[gpui::test]
fn exact_region_fields_are_named_validate_and_release_with_the_page(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    handle.update(cx, |this, _, _| {
        this.output_directory = Some(std::env::temp_dir());
        this.xcap_displays = vec![exact_display()];
    }).unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut("3"), cx);
        window.render_frame(cx);
        let _ = window.find_by_label("Region width");
        window.click("exact-region-exact-fixture", cx);
    }).unwrap();
    cx.run_until_parked();
    handle.update(cx, |this, _, _| {
        assert!(this.exact_region_status.starts_with("The region must"));
        assert!(this.exact_region_inputs.is_some());
        assert!(!this.capturing && this.last_capture.is_none() && this.last_saved.is_none());
    }).unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.press(&shortcut("1"), cx)).unwrap();
    handle.update(cx, |this, _, cx| {
        assert!(this.exact_region_inputs.is_none());
        assert!(this.exact_region_status.is_empty());
        this.capture_exact_region(exact_display(), cx);
        assert!(!this.capturing);
    }).unwrap();
}

#[gpui::test]
fn region_presets_load_without_capture_reject_stale_actions_and_preserve_failed_saves(cx: &mut TestAppContext) {
    use gpui::test::TestEventExt as _;
    let handle = open_workspace(cx);
    let blocker = std::env::temp_dir().join(format!("screenshots-preset-blocker-{}", std::process::id()));
    std::fs::write(&blocker, b"not a folder").unwrap();
    let preset = app_core::RegionPreset::new("Small", app_core::PixelRegion { x: 10, y: 20, width: 30, height: 40 }).unwrap();
    handle.update(cx, |this, _, _| {
        this.output_directory = Some(blocker.clone());
        this.xcap_displays = vec![exact_display()];
        this.region_presets = vec![preset.clone()];
    }).unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut("3"), cx);
        window.render_frame(cx);
        let bounds = window.find_by_label("Use region preset Small").bounds();
        window.pointer_down(bounds.center(), gpui::MouseButton::Left, cx);
        window.pointer_up(bounds.center(), gpui::MouseButton::Left, cx);
    }).unwrap();
    handle.update(cx, |this, window, cx| {
        let values: Vec<_> = this.exact_region_inputs.as_ref().unwrap().iter().map(|input| input.read(cx).value().to_string()).collect();
        assert_eq!(values, ["10", "20", "30", "40"]);
        assert_eq!(this.region_preset_name.as_ref().unwrap().read(cx).value().as_ref(), "Small");
        assert!(!this.capturing && this.last_capture.is_none());
        this.region_presets[0].x = 11;
        this.use_region_preset(&preset, window, cx);
        this.remove_region_preset(&preset, cx);
        assert_eq!(this.region_presets[0].x, 11);
        assert!(!this.settings_saving);
        this.save_region_preset(exact_display(), cx);
    }).unwrap();
    cx.run_until_parked();
    handle.update(cx, |this, _, _| {
        assert!(!this.settings_saving);
        assert_eq!(this.region_presets[0].x, 11);
        assert!(this.exact_region_status.starts_with("Region presets unchanged:"));
        assert!(!this.capturing && this.last_saved.is_none());
    }).unwrap();
    cx.update_window(handle.into(), |_, window, cx| window.press(&shortcut("1"), cx)).unwrap();
    handle.update(cx, |this, _, _| assert!(this.region_preset_name.is_none() && this.exact_region_inputs.is_none())).unwrap();
    std::fs::remove_file(blocker).unwrap();
}

#[gpui::test]
fn open_image_cancel_and_invalid_input_release_admission(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    handle.update(cx, |this, _, _| this.output_directory = Some(std::env::temp_dir())).unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("open-image", cx);
    }).unwrap();
    assert!(cx.did_prompt_for_paths());
    handle.update(cx, |this, _, cx| {
        this.open_image(cx);
        assert!(this.capturing);
    }).unwrap();
    #[cfg(target_os = "windows")]
    cx.update_window(handle.into(), |_, window, cx| {
        window.press("ctrl-shift-v", cx);
    }).unwrap();
    cx.update(|cx| assert_eq!(handle.read(cx).unwrap().capture_status, "Choosing an image…"));
    cx.simulate_path_prompt_response(|options| {
        assert!(options.files && !options.directories && !options.multiple);
        None
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert!(!state.capturing);
        assert_eq!(state.capture_status, "Image selection canceled.");
        assert!(state.editor.is_none() && state.last_capture.is_none() && state.last_saved.is_none());
    });
    cx.update_window(handle.into(), |_, window, cx| window.press(&shortcut("o"), cx)).unwrap();
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("missing-image.png")]));
    cx.run_until_parked();
    cx.update(|cx| {
        let state = handle.read(cx).unwrap();
        assert!(!state.capturing);
        assert!(state.capture_status.starts_with("Could not load image:"));
        assert!(state.editor.is_none() && state.last_capture.is_none() && state.last_saved.is_none());
    });
    handle.update(cx, |this, _, cx| this.open_image(cx)).unwrap();
    handle.update(cx, |this, _, _| this.quitting = true).unwrap();
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
        .save_with_format(&path, image::ImageFormat::Jpeg).unwrap();
    let original = std::fs::read(&path).unwrap();
    handle.update(cx, |this, _, cx| {
        this.output_directory = Some(root.clone());
        this.open_image(cx);
    }).unwrap();
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
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&root).unwrap();
}

#[gpui::test]
fn library_open_image_opens_webp_without_importing_or_writing_output(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!("screenshots-library-open-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("original.webp");
    let pixels = image::RgbaImage::from_pixel(3, 2, image::Rgba([12, 34, 56, 78]));
    pixels.save(&path).unwrap();
    let original = std::fs::read(&path).unwrap();
    handle.update(cx, |this, _, _| this.output_directory = Some(root.clone())).unwrap();
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut("2"), cx);
        window.render_frame(cx);
        window.click("library-open-image", cx);
    }).unwrap();
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|options| {
        assert!(!options.multiple && options.files && !options.directories);
        Some(vec![path.clone()])
    });
    cx.run_until_parked();
    handle.update(cx, |this, _, cx| {
        assert_eq!(this.page, ControlPage::Captures);
        assert!(!this.history_visible && this.history_search_input.is_none());
        assert!(!this.capturing && this.last_capture.is_none() && this.last_saved.is_none());
        assert!(this.history_entries.is_empty());
        let editor = this.editor.unwrap().read(cx).unwrap();
        assert_eq!(&*editor.source.pixels, &pixels);
    }).unwrap();
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
    }).unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn file_drop_validates_admission_and_opens_one_image_without_output(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let root = std::env::temp_dir().join(format!("screenshots-drop-image-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("original.webp");
    image::RgbaImage::from_pixel(3, 2, image::Rgba([12, 34, 56, 78])).save(&path).unwrap();
    let original = std::fs::read(&path).unwrap();
    handle.update(cx, |this, _, _| {
        this.output_directory = Some(root.clone());
        this.capturing = true;
        this.capture_status = "Existing capture".into();
    }).unwrap();
    drop_paths(handle, std::slice::from_ref(&path), cx);
    handle.update(cx, |this, _, _| {
        assert_eq!(this.capture_status, "Existing capture");
        assert!(this.editor.is_none());
        this.capturing = false;
    }).unwrap();
    drop_paths(handle, &[path.clone(), path.clone()], cx);
    handle.update(cx, |this, _, _| {
        assert!(this.capture_status.starts_with("Drop exactly one"));
        assert!(!this.capturing && this.editor.is_none());
    }).unwrap();
    drop_paths(handle, &[root.join("missing.png")], cx);
    handle.update(cx, |this, _, _| {
        assert!(this.capture_status.starts_with("Could not load image:"));
        assert!(!this.capturing && this.editor.is_none());
    }).unwrap();
    drop_paths(handle, std::slice::from_ref(&root), cx);
    handle.update(cx, |this, _, _| {
        assert!(this.capture_status.starts_with("Could not load image:"));
        assert!(!this.capturing && this.editor.is_none());
    }).unwrap();
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
fn output_profile_failure_preserves_active_choice_and_rejects_multi_image_output(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    let blocker = std::env::temp_dir().join(format!("screenshots-profile-blocker-{}", std::process::id()));
    std::fs::write(&blocker, b"not a directory").unwrap();
    cx.update(|cx| handle.update(cx, |this, _, _| {
        this.output_directory = Some(blocker.clone());
    }).unwrap());
    cx.update_window(handle.into(), |_, window, cx| {
        window.press(&shortcut(","), cx);
        window.render_frame(cx);
        window.within("capture-output-profile").click(0usize, cx);
    }).unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let view = handle.read(cx).unwrap();
        assert!(!view.settings_saving);
        assert_eq!(view.output_profile, CaptureOutputProfile::SaveAndCopy);
        assert!(view.settings_status.starts_with("Capture output unchanged:"));
    });
    cx.update(|cx| handle.update(cx, |this, _, cx| {
        this.output_profile = CaptureOutputProfile::CopyOnly;
        this.start_capture(vec![CaptureJob::Display("first".into()), CaptureJob::Display("second".into())], cx);
        assert!(!this.capturing);
        assert!(this.capture_status.starts_with("Choose one display"));
        assert!(this.last_capture.is_none());
        assert!(!this.copying && !this.saving);
    }).unwrap());
    std::fs::remove_file(blocker).unwrap();
}

#[cfg(target_os = "windows")]
#[gpui::test]
fn repeat_without_a_region_shows_capture_guidance_without_output(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    handle.update(cx, |state, _, cx| {
        state.output_directory = Some(std::env::temp_dir());
        state.show_page(ControlPage::Settings, cx);
        state.repeat_last_region(cx);
        assert_eq!(state.page, ControlPage::Captures);
        assert!(state.capture_status.starts_with("Select a region"));
        assert!(state.last_region.is_none() && state.last_capture.is_none());
        assert!(!state.capturing && !state.copying && !state.saving);
    }).unwrap();
}

#[cfg(target_os = "windows")]
#[gpui::test]
fn active_window_countdown_can_cancel_without_starting_capture(cx: &mut TestAppContext) {
    let handle = open_workspace(cx);
    cx.update(|cx| handle.update(cx, |this, _, cx| {
        this.output_directory = Some(std::env::temp_dir());
        this.start_delayed_capture(DelayedCapture::ActiveWindow, cx);
        assert!(this.capture_delay.is_some());
        assert!(this.capture_status.contains("Focus the target window"));
        this.cancel_capture_delay(cx);
    }).unwrap());
    cx.executor().advance_clock(std::time::Duration::from_secs(4));
    cx.run_until_parked();
    cx.update(|cx| {
        let view = handle.read(cx).unwrap();
        assert!(view.capture_delay.is_none());
        assert!(!view.capturing && !view.copying && !view.saving);
        assert!(view.last_capture.is_none() && view.editor.is_none());
        assert_eq!(view.capture_status, "Delayed capture canceled");
    });
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
            assert_eq!(window.find(("capture-hotkey", index)).value(), Some(expected.as_str()));
        }
        // Isolated UI tests own no native hotkey manager or settings destination.
        assert_eq!(window.find("save-hotkeys").disabled(), Some(true));
        window.press(&shortcut("1"), cx);
        window.render_frame(cx);
    }).unwrap();
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
                display_id: "fixture".into(), window_id: None, width: 4, height: 3,
                pixels: Arc::new(image::RgbaImage::new(4, 3)), png_path: PathBuf::from("unused-source.png"),
                elapsed_ms: 0, source_format: app_core::PixelFormat::Rgba8UnormSrgb,
                display_geometry: Some(app_core::DisplayGeometry { x: -4, y: 0, width: 4, height: 3, scale_factor: 1.25, rotation: 0.0 }),
            };
            state.set_last_region(Some(app_core::RememberedRegion::from_snapshot(&source, app_core::PixelRegion { x: 1, y: 1, width: 2, height: 2 }).unwrap()));
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
        assert_eq!(state.last_region.as_ref().unwrap().area(), app_core::PixelRegion { x: 1, y: 1, width: 2, height: 2 });
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
