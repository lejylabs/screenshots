use anyhow::{Result, ensure};
use std::path::{Path, PathBuf};

pub struct ExportReport {
    pub exported: usize,
    pub failures: Vec<(PathBuf, String)>,
    pub not_exported: usize,
}

/// Copy at most one selected history page. Completed files survive later failure/cancellation.
pub fn export_saved_pngs(
    sources: &[PathBuf],
    directory: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<ExportReport> {
    export_saved_pngs_with_metadata(sources, directory, false, cancelled)
}

/// Explicit metadata policy; the default byte-preserving export remains unchanged.
pub fn export_saved_pngs_with_metadata(
    sources: &[PathBuf],
    directory: &Path,
    strip_metadata: bool,
    cancelled: impl Fn() -> bool,
) -> Result<ExportReport> {
    ensure!(
        !sources.is_empty() && sources.len() <= 20,
        "Select 1 to 20 images"
    );
    let mut report = ExportReport {
        exported: 0,
        failures: Vec::new(),
        not_exported: 0,
    };
    if cancelled() {
        report.not_exported = sources.len();
        return Ok(report);
    }
    ensure!(
        directory.is_absolute() && directory.is_dir(),
        "Choose an existing absolute export folder"
    );
    for (index, source) in sources.iter().enumerate() {
        if cancelled() {
            report.not_exported = sources.len() - index;
            break;
        }
        let result = (|| -> Result<()> {
            ensure!(source.is_absolute(), "Image path must be absolute");
            let name = source
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("Invalid image file name"))?;
            #[cfg(target_os = "windows")]
            ensure!(
                !name.to_string_lossy().contains(':'),
                "Alternate data streams cannot be exported"
            );
            if strip_metadata {
                export_clean_png(source, &directory.join(name), &cancelled)?;
            } else {
                crate::capture::export_saved_png_cancellable(
                    source,
                    &directory.join(name),
                    &cancelled,
                )?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => report.exported += 1,
            Err(error)
                if error
                    .downcast_ref::<crate::CaptureError>()
                    .is_some_and(|error| matches!(error, crate::CaptureError::Canceled)) =>
            {
                report.not_exported = sources.len() - index;
                break;
            }
            Err(error) => report.failures.push((source.clone(), format!("{error:#}"))),
        }
    }
    Ok(report)
}

fn export_clean_png(source: &Path, destination: &Path, cancelled: impl Fn() -> bool) -> Result<()> {
    use image::ImageEncoder;
    let started = std::time::Instant::now();
    let check = || {
        if cancelled() {
            Err(crate::CaptureError::Canceled)
        } else if started.elapsed() > std::time::Duration::from_secs(60) {
            Err(crate::CaptureError::Backend(
                "PNG export deadline exceeded".into(),
            ))
        } else {
            Ok(())
        }
    };
    check()?;
    ensure!(!destination.try_exists()?, "Export output already exists");
    // The shared file loader freezes the encoded extent and applies EXIF orientation.
    let pixels = crate::clipboard::load_export_png(source)?;
    check()?;
    crate::capture::write_output_checked(
        destination,
        |file| {
            let mut writer = std::io::BufWriter::with_capacity(64 * 1024, file);
            image::codecs::png::PngEncoder::new_with_quality(
                &mut writer,
                image::codecs::png::CompressionType::Fast,
                image::codecs::png::FilterType::Adaptive,
            )
            .write_image(
                pixels.as_raw(),
                pixels.width(),
                pixels.height(),
                image::ExtendedColorType::Rgba8,
            )?;
            std::io::Write::flush(&mut writer)?;
            Ok(())
        },
        check,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn export_policies_reject_named_pipe_sources_without_reading() -> Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let root =
            std::env::temp_dir().join(format!("screenshots-pipe-export-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root)?;
        let source = root.join("pipe.png");
        let name = std::ffi::CString::new(source.as_os_str().as_bytes())?;
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        for strip in [false, true] {
            let report = export_saved_pngs_with_metadata(
                std::slice::from_ref(&source),
                &root,
                strip,
                || false,
            )?;
            assert_eq!(report.exported, 0);
            assert_eq!(report.failures.len(), 1);
        }
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn metadata_policy_reencodes_pixels_and_preserves_originals() -> Result<()> {
        use image::ImageEncoder;
        let root =
            std::env::temp_dir().join(format!("screenshots-clean-export-{}", uuid::Uuid::new_v4()));
        let original_dir = root.join("original");
        let clean_dir = root.join("clean");
        std::fs::create_dir_all(&original_dir)?;
        std::fs::create_dir(&clean_dir)?;
        let source = root.join("input.png");
        let pixels = image::RgbaImage::from_pixel(3, 2, image::Rgba([13, 24, 35, 87]));
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
        encoder.set_exif_metadata(vec![b'S'; 128])?;
        encoder.write_image(pixels.as_raw(), 3, 2, image::ExtendedColorType::Rgba8)?;
        std::fs::write(&source, &bytes)?;
        assert!(bytes.windows(4).any(|w| w == b"eXIf"));
        let copied = export_saved_pngs(std::slice::from_ref(&source), &original_dir, || false)?;
        assert_eq!(copied.exported, 1);
        assert_eq!(std::fs::read(original_dir.join("input.png"))?, bytes);
        let clean = export_saved_pngs_with_metadata(
            std::slice::from_ref(&source),
            &clean_dir,
            true,
            || false,
        )?;
        assert_eq!(clean.exported, 1);
        assert!(clean.failures.is_empty());
        let cleaned = std::fs::read(clean_dir.join("input.png"))?;
        assert!(!cleaned.windows(4).any(|w| w == b"eXIf"));
        assert_eq!(image::load_from_memory(&cleaned)?.into_rgba8(), pixels);
        assert_eq!(std::fs::read(&source)?, bytes);
        let duplicate = export_saved_pngs_with_metadata(
            std::slice::from_ref(&source),
            &clean_dir,
            true,
            || false,
        )?;
        assert_eq!(duplicate.exported, 0);
        assert_eq!(duplicate.failures.len(), 1);
        let canceled = export_saved_pngs_with_metadata(
            std::slice::from_ref(&source),
            &clean_dir,
            true,
            || true,
        )?;
        assert_eq!(canceled.not_exported, 1);
        let cancel_dir = root.join("cancel");
        std::fs::create_dir(&cancel_dir)?;
        let checks = std::cell::Cell::new(0);
        let canceled = export_saved_pngs_with_metadata(
            std::slice::from_ref(&source),
            &cancel_dir,
            true,
            || {
                checks.set(checks.get() + 1);
                checks.get() >= 5
            },
        )?;
        assert_eq!((canceled.exported, canceled.not_exported), (0, 1));
        assert_eq!(std::fs::read_dir(&cancel_dir)?.count(), 0);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn exports_report_partial_results_without_overwriting_or_changing_sources() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-export-{}", uuid::Uuid::new_v4()));
        let directory = root.join("out");
        assert!(export_saved_pngs(&[], &directory, || false).is_err());
        assert!(
            export_saved_pngs(&vec![root.join("image.png"); 21], &directory, || false).is_err()
        );
        let stopped = export_saved_pngs(&[root.join("image.png")], &directory, || true)?;
        assert_eq!((stopped.exported, stopped.not_exported), (0, 1));
        assert!(!root.exists());
        std::fs::create_dir_all(&directory)?;
        let source = root.join("first.png");
        image::RgbaImage::new(2, 3).save(&source)?;
        let original = std::fs::read(&source)?;
        let missing = root.join("missing.png");
        let invalid = root.join("invalid.png");
        std::fs::write(&invalid, b"not PNG")?;
        let report = export_saved_pngs(
            &[source.clone(), missing, invalid.clone()],
            &directory,
            || false,
        )?;
        assert_eq!(
            (report.exported, report.failures.len(), report.not_exported),
            (1, 2, 0)
        );
        assert_eq!(std::fs::read(directory.join("first.png"))?, original);
        assert_eq!(std::fs::read(&source)?, original);
        let conflict = export_saved_pngs(&[source], &directory, || false)?;
        assert_eq!((conflict.exported, conflict.failures.len()), (0, 1));
        assert_eq!(std::fs::read(directory.join("first.png"))?, original);
        assert_eq!(std::fs::read(&invalid)?, b"not PNG");
        assert_eq!(std::fs::read_dir(&directory)?.count(), 1);
        assert!(!root.join("history.sqlite3").exists());
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn cancellation_keeps_completed_copies_and_removes_unpublished_parts() -> Result<()> {
        let root = std::env::temp_dir().join(format!(
            "screenshots-export-cancel-{}",
            uuid::Uuid::new_v4()
        ));
        let directory = root.join("out");
        std::fs::create_dir_all(&directory)?;
        let first = root.join("first.png");
        let second = root.join("second.png");
        image::RgbaImage::new(2, 3).save(&first)?;
        image::RgbaImage::new(3, 2).save(&second)?;
        let report = export_saved_pngs(&[first.clone(), second], &directory, || {
            directory.join("first.png").exists()
        })?;
        assert_eq!(
            (report.exported, report.failures.len(), report.not_exported),
            (1, 0, 1)
        );
        assert!(!directory.join("second.png").exists());
        let large = root.join("large.png");
        let mut encoded = vec![137, 80, 78, 71, 13, 10, 26, 10];
        encoded.resize(256 * 1024, 0);
        std::fs::write(&large, &encoded)?;
        let report = export_saved_pngs(&[large.clone()], &directory, || {
            std::fs::read_dir(&directory)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "part")
                        && entry
                            .metadata()
                            .is_ok_and(|metadata| metadata.len() > 64 * 1024)
                })
        })?;
        assert_eq!(
            (report.exported, report.failures.len(), report.not_exported),
            (0, 0, 1)
        );
        assert_eq!(std::fs::read_dir(&directory)?.count(), 1);
        assert_eq!(std::fs::read(&large)?, encoded);
        assert_eq!(
            std::fs::read(directory.join("first.png"))?,
            std::fs::read(&first)?
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    #[ignore = "short optimized comparison of 20 durable small-file copies; synthetic PNGs only"]
    fn selected_export_cost() -> Result<()> {
        let root =
            std::env::temp_dir().join(format!("screenshots-export-cost-{}", uuid::Uuid::new_v4()));
        let individual = root.join("individual");
        let batch = root.join("batch");
        std::fs::create_dir_all(&individual)?;
        std::fs::create_dir_all(&batch)?;
        let source = root.join("source.png");
        image::RgbaImage::from_fn(1024, 768, |x, y| {
            image::Rgba([(x % 251) as u8, (y % 251) as u8, ((x + y) % 251) as u8, 255])
        })
        .save(&source)?;
        let mut sources = Vec::new();
        for index in 0..20 {
            let path = root.join(format!("source-{index}.png"));
            std::fs::copy(&source, &path)?;
            sources.push(path);
        }
        let started = std::time::Instant::now();
        for path in &sources {
            crate::export_saved_png(path, &individual.join(path.file_name().unwrap()))?;
        }
        let baseline = started.elapsed();
        let started = std::time::Instant::now();
        let report = export_saved_pngs(&sources, &batch, || false)?;
        let elapsed = started.elapsed();
        assert_eq!(
            (report.exported, report.failures.len(), report.not_exported),
            (20, 0, 0)
        );
        eprintln!(
            "20 durable PNG copies: individual={baseline:?}; selected export={elapsed:?}; source={} bytes; one 64 KiB streaming buffer, no image decoding",
            std::fs::metadata(&source)?.len()
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
