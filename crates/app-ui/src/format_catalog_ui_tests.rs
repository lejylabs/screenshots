use super::*;

#[gpui::test]
fn capture_library_export_and_settings_backup_share_an_explicit_private_catalog(
    cx: &mut TestAppContext,
) {
    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let root = std::env::temp_dir().join(format!(
        "screenshots-ui-catalog-{}",
        app_core::capture_file_name("Fixture").unwrap().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    let fixture = Fixture(root.canonicalize().unwrap());
    let catalog_path = fixture.0.join("private-formats.json");
    let mut formats =
        app_core::CaptureFormatSettings::for_policy(app_core::CaptureFormatPolicy::WebpLossless);
    formats.library_export = app_core::LibraryExportPolicy::Convert {
        encoding: app_core::CaptureFormatPolicy::Png,
    };
    formats.save_at(&catalog_path).unwrap();
    let original_catalog = std::fs::read(&catalog_path).unwrap();
    let handle = open_workspace(cx);
    let pixels = Arc::new(image::RgbaImage::from_pixel(
        3,
        2,
        image::Rgba([17, 41, 83, 128]),
    ));
    let shot = CapturedImage {
        desktop_layout: None,
        display_id: "private-format-ui-fixture".into(),
        window_id: None,
        width: 3,
        height: 2,
        pixels: pixels.clone(),
        png_path: fixture.0.join("source.png"),
        elapsed_ms: 0,
        source_format: app_core::PixelFormat::Rgba8UnormSrgb,
        display_geometry: None,
    };
    handle
        .update(cx, |this, _, cx| {
            this.format_catalog = crate::format_catalog::FormatCatalog::at(catalog_path.clone());
            this.output_directory = Some(fixture.0.clone());
            this.history_database = Some(fixture.0.join("history.sqlite3"));
            this.last_capture = Some(shot.clone());
            this.output_captures(vec![shot.clone()], false, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let saved = handle
        .update(cx, |this, _, _| {
            assert!(!this.saving && this.last_capture.is_none());
            assert_eq!(this.clipboard_status, "No capture yet");
            this.last_saved.clone().unwrap()
        })
        .unwrap();
    assert_eq!(saved.extension().unwrap(), "webp");
    assert!(!shot.png_path.exists());
    assert_eq!(image::open(&saved).unwrap().into_rgba8(), *pixels);
    let original_image = std::fs::read(&saved).unwrap();

    let directory = fixture.0.join("exported");
    std::fs::create_dir(&directory).unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.saving = true;
            this.begin_history_export(
                vec![saved.clone()],
                directory.clone(),
                this.history_revision,
                cx,
            );
        })
        .unwrap();
    cx.run_until_parked();
    let converted = directory.join("source.png");
    assert_eq!(image::open(&converted).unwrap().into_rgba8(), *pixels);
    assert_eq!(std::fs::read(&saved).unwrap(), original_image);

    handle
        .update(cx, |this, _, cx| this.export_settings(cx))
        .unwrap();
    let backup = fixture.0.join("backup.json");
    cx.simulate_new_path_selection(|_| Some(backup.clone()));
    cx.run_until_parked();
    let (_, _, backed_up) =
        app_core::CaptureSettings::import_with_formats(&backup, fixture.0.clone()).unwrap();
    assert_eq!(backed_up, Some(formats));
    assert_eq!(std::fs::read(&catalog_path).unwrap(), original_catalog);
    assert_eq!(std::fs::read(&saved).unwrap(), original_image);
}

#[gpui::test]
fn library_load_cancels_inside_its_real_reader_and_late_delivery_cannot_replace_a_new_editor(
    cx: &mut TestAppContext,
) {
    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let root = std::env::temp_dir().join(format!(
        "screenshots-library-cancel-{}",
        app_core::capture_file_name("Fixture").unwrap().unwrap()
    ));
    std::fs::create_dir(&root).unwrap();
    let fixture = Fixture(root.canonicalize().unwrap());
    let old_path = fixture.0.join("old.png");
    let new_path = fixture.0.join("new.png");
    image::RgbaImage::from_pixel(3, 2, image::Rgba([11, 23, 37, 128]))
        .save(&old_path)
        .unwrap();
    let expected = image::RgbaImage::from_pixel(3, 2, image::Rgba([101, 103, 107, 255]));
    expected.save(&new_path).unwrap();
    let original = std::fs::read(&old_path).unwrap();
    let checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let handle = open_workspace(cx);
    let (old_generation, old_cancel) = handle
        .update(cx, |this, _, cx| {
            this.output_directory = Some(fixture.0.clone());
            this.test_library_load_cancel = Some((checks.clone(), 6));
            this.open_history_editor(old_path.clone(), cx);
            (
                this.capture_generation,
                this.native_capture_cancel.clone().unwrap(),
            )
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(checks.load(std::sync::atomic::Ordering::Acquire), 6);
    assert!(old_cancel.check().is_err());
    handle
        .update(cx, |this, _, cx| {
            assert!(!this.capturing && this.editor.is_none());
            assert!(this.last_capture.is_none() && this.last_saved.is_none());
            assert_eq!(this.capture_status, "Image opening canceled.");
            this.test_library_load_cancel = None;
            this.open_history_editor(new_path.clone(), cx);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .update(cx, |this, _, cx| {
            let status = this.capture_status.clone();
            assert_ne!(this.capture_generation, old_generation);
            this.deliver_image_input(
                old_generation,
                &old_cancel,
                Err("obsolete private fixture delivery".into()),
                cx,
            );
            assert_eq!(this.capture_status, status);
            assert!(!this.capturing && this.last_capture.is_none() && this.last_saved.is_none());
            assert_eq!(
                this.editor
                    .unwrap()
                    .read(cx)
                    .unwrap()
                    .source
                    .pixels
                    .as_ref(),
                &expected
            );
            assert_eq!(this.clipboard_status, "No capture yet");
        })
        .unwrap();
    assert_eq!(std::fs::read(&old_path).unwrap(), original);
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 2);
}
