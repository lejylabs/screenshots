use super::*;
use std::{cell::Cell, sync::Arc};

#[test]
fn canceled_file_encoders_remove_unpublished_parts_for_every_existing_format() {
    let root = std::env::temp_dir().join(format!(
        "screenshots-cancel-encode-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&root).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let mut seed = 0x12345678u32;
    let pixels = RgbaImage::from_fn(512, 128, |_, _| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        image::Rgba([seed as u8, (seed >> 8) as u8, (seed >> 16) as u8, 255])
    });
    let input = frame(pixels.clone());
    for extension in ["png", "jpg", "webp"] {
        let destination = root.join(format!("out.{extension}"));
        let canceled = || {
            std::fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| {
                    entry.file_name().to_string_lossy().ends_with(".part")
                        && entry.metadata().unwrap().len() > 0
                })
        };
        assert!(matches!(
            input.save_image_as_with_cancel(
                &destination,
                90,
                crate::ExportSize::Original,
                canceled
            ),
            Err(CaptureError::Canceled)
        ));
        assert!(!destination.exists());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        assert!(input.pixels.as_ref() == &pixels);
    }
}

#[test]
fn encoder_cancellation_is_an_error_that_write_all_will_not_retry() {
    use std::{
        io::Write,
        sync::atomic::{AtomicUsize, Ordering},
    };
    struct Sink(Arc<AtomicUsize>);
    impl Write for Sink {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.fetch_add(data.len(), Ordering::Relaxed);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let written = Arc::new(AtomicUsize::new(0));
    let canceled = || written.load(Ordering::Relaxed) > 0;
    let mut writer =
        crate::image_cancel::CheckedImageWriter::new(Sink(written.clone()), &canceled).unwrap();
    assert!(writer.write(&vec![0; 128 * 1024]).unwrap() > 0);
    let error = writer.write(&[0]).unwrap_err();
    assert_ne!(error.kind(), std::io::ErrorKind::Interrupted);
}

fn source(width: u32, height: u32) -> RgbaImage {
    RgbaImage::from_fn(width, height, |x, y| {
        image::Rgba([x as u8, y as u8, (x ^ y) as u8, ((x + y) % 3 * 127) as u8])
    })
}

#[test]
#[ignore = "Optimized synthetic 4K copy/render development observation"]
fn cancellable_raster_development_timings() {
    use std::{
        hint::black_box,
        sync::atomic::{AtomicBool, Ordering},
        time::Instant,
    };
    let input = frame(source(3840, 2160));
    let flag = AtomicBool::new(false);
    let region = crate::PixelRegion {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    };
    let mut samples = [Vec::new(), Vec::new(), Vec::new()];
    for round in 0..36 {
        let order = [
            [0, 1, 2],
            [1, 2, 0],
            [2, 0, 1],
            [0, 2, 1],
            [2, 1, 0],
            [1, 0, 2],
        ][round % 6];
        let mut expected = input.pixels.as_ref().clone();
        expected.put_pixel(0, 0, image::Rgba([0, 0, 0, 255]));
        for mode in order {
            let started = Instant::now();
            let result = match mode {
                0 => {
                    let mut p = input.pixels.as_ref().clone();
                    p.put_pixel(0, 0, image::Rgba([0, 0, 0, 255]));
                    p
                }
                1 => Arc::try_unwrap(
                    crate::render_ordered_snapshot(&input, &[region], None, &[], &[], None, &[])
                        .unwrap()
                        .pixels,
                )
                .unwrap(),
                _ => Arc::try_unwrap(
                    crate::render_ordered_snapshot_with_cancel(
                        &input,
                        &[region],
                        None,
                        &[],
                        &[],
                        None,
                        &[],
                        || flag.load(Ordering::Relaxed),
                    )
                    .unwrap()
                    .pixels,
                )
                .unwrap(),
            };
            black_box(&result);
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            assert!(result == expected);
            if round >= 6 {
                samples[mode].push(elapsed);
            }
        }
    }
    let mut summary = Vec::new();
    for values in &samples {
        let mut sorted = values.clone();
        sorted.sort_by(f64::total_cmp);
        summary.push(serde_json::json!({"median_ms":(sorted[14]+sorted[15])/2.0,"p95_ms":sorted[28],"samples_ms":values}));
    }
    println!(
        "RASTER_TIMING {}",
        serde_json::json!({"width":3840,"height":2160,"bytes":input.pixels.as_raw().len(),"copy_reference":summary[0],"default_render":summary[1],"cancellable_render":summary[2],"scope":"warm synthetic CPU copy plus one opaque redaction; not presentation/private memory/hardware acceptance"})
    );
}

fn frame(pixels: RgbaImage) -> crate::CapturedImage {
    crate::CapturedImage {
        display_id: "fixture".into(),
        window_id: None,
        width: pixels.width(),
        height: pixels.height(),
        pixels: Arc::new(pixels),
        png_path: "never-written.png".into(),
        elapsed_ms: 0,
        source_format: crate::PixelFormat::Rgba8UnormSrgb,
        display_geometry: None,
    }
}

#[test]
fn exact_transforms_match_the_preexisting_image_crate_operations() {
    for (width, height) in [(1, 1), (1, 19), (19, 1), (7, 6), (32, 17)] {
        let pixels = source(width, height);
        for (degrees, expected) in [
            (90, image::imageops::rotate90(&pixels)),
            (180, image::imageops::rotate180(&pixels)),
            (270, image::imageops::rotate270(&pixels)),
        ] {
            assert_eq!(
                rotate_rgba_with_cancel(pixels.clone(), degrees, || false).unwrap(),
                expected
            );
        }
        assert_eq!(
            flip_rgba_with_cancel(pixels.clone(), true, || false).unwrap(),
            image::imageops::flip_horizontal(&pixels)
        );
        assert_eq!(
            flip_rgba_with_cancel(pixels.clone(), false, || false).unwrap(),
            image::imageops::flip_vertical(&pixels)
        );
    }
}

#[test]
fn nearest_resize_and_canvas_keep_known_hidden_rgb_and_straight_alpha() {
    let pixels = RgbaImage::from_raw(2, 1, vec![11, 22, 33, 0, 255, 0, 0, 128]).unwrap();
    let resized = resize_nearest_rgba_with_cancel(&pixels, 4, 2, || false).unwrap();
    for y in 0..2 {
        assert_eq!(resized.get_pixel(0, y).0, [11, 22, 33, 0]);
        assert_eq!(resized.get_pixel(1, y).0, [11, 22, 33, 0]);
        assert_eq!(resized.get_pixel(2, y).0, [255, 0, 0, 128]);
        assert_eq!(resized.get_pixel(3, y).0, [255, 0, 0, 128]);
    }
    let output = canvas_rgba_with_cancel(&pixels, 1, [0, 0, 255, 128], || false).unwrap();
    assert_eq!(output.dimensions(), (4, 3));
    assert_eq!(output.get_pixel(1, 1).0, [0, 0, 255, 128]);
    assert_eq!(output.get_pixel(2, 1).0, [170, 0, 85, 192]);
    assert_eq!(output.get_pixel(0, 0).0, [0, 0, 255, 128]);
}

#[test]
fn raster_work_cancels_within_wide_spans_and_preserves_borrowed_sources() {
    for (width, height) in [(131073, 1), (1, 131073)] {
        let pixels = source(width, height);
        let before = pixels.clone();
        let calls = Cell::new(0);
        let canceled = || {
            calls.set(calls.get() + 1);
            calls.get() >= 4
        };
        let result = crate::remove_image_strip_with_cancel(&pixels, width > 1, 1, 1, canceled);
        assert!(matches!(result, Err(CaptureError::Canceled)));
        assert_eq!(pixels, before);
        assert_eq!(calls.get(), 4);
    }
}

#[test]
fn narrow_copy_shares_a_budget_across_rows_and_reduction_checks_inside_one_output_pixel() {
    let input = frame(source(2, 65536));
    let calls = Cell::new(0);
    let result = crate::render_ordered_snapshot_with_cancel(
        &input,
        &[],
        Some(crate::PixelRegion {
            x: 1,
            y: 0,
            width: 1,
            height: 65536,
        }),
        &[],
        &[],
        None,
        &[],
        || {
            calls.set(calls.get() + 1);
            false
        },
    )
    .unwrap();
    assert_eq!(
        *result.pixels,
        image::imageops::crop_imm(input.pixels.as_ref(), 1, 0, 1, 65536).to_image()
    );
    assert!(
        calls.get() <= 8,
        "Short rows must share one work budget, observed {} checks",
        calls.get()
    );
    let interrupted = Cell::new(0);
    let canceled = crate::render_ordered_snapshot_with_cancel(
        &input,
        &[],
        Some(crate::PixelRegion {
            x: 1,
            y: 0,
            width: 1,
            height: 65536,
        }),
        &[],
        &[],
        None,
        &[],
        || {
            interrupted.set(interrupted.get() + 1);
            interrupted.get() >= 3
        },
    )
    .unwrap_err();
    assert!(matches!(
        canceled.downcast_ref::<CaptureError>(),
        Some(CaptureError::Canceled)
    ));
    assert_eq!(interrupted.get(), 3);
    let pixels = source(1024, 1024);
    let calls = Cell::new(0);
    assert!(matches!(
        crate::downscale_export_with_cancel(&pixels, crate::ExportSize::Exact(1, 1), || {
            calls.set(calls.get() + 1);
            calls.get() >= 3
        }),
        Err(CaptureError::Canceled)
    ));
    assert_eq!(calls.get(), 3);
}

#[test]
fn ordered_rendering_propagates_cancellation_into_blur_work() {
    let input = frame(source(64, 1024));
    let all = crate::PixelRegion {
        x: 0,
        y: 0,
        width: 64,
        height: 1024,
    };
    let copy_checks = Cell::new(0);
    crate::render_ordered_snapshot_with_cancel(&input, &[], Some(all), &[], &[], None, &[], || {
        copy_checks.set(copy_checks.get() + 1);
        false
    })
    .unwrap();
    let cancel_at = copy_checks.get() + 1;
    let calls = Cell::new(0);
    let result = crate::render_ordered_snapshot_with_cancel(
        &input,
        &[],
        None,
        &[],
        &[],
        None,
        &[all],
        || {
            calls.set(calls.get() + 1);
            calls.get() >= cancel_at
        },
    );
    assert!(matches!(
        result.unwrap_err().downcast_ref::<CaptureError>(),
        Some(CaptureError::Canceled)
    ));
    assert_eq!(calls.get(), cancel_at);
}

#[test]
fn every_png_cancellation_boundary_including_explicit_flush_returns_typed_canceled() {
    let input = frame(source(7, 5));
    let original = input.pixels.clone();
    let observed = Cell::new(0);
    let encoded = input
        .png_bytes_with_cancel(|| {
            observed.set(observed.get() + 1);
            false
        })
        .unwrap();
    assert_eq!(
        image::load_from_memory(&encoded).unwrap().into_rgba8(),
        *original
    );
    assert!(observed.get() >= 4);
    for cancel_at in 1..=observed.get() {
        let calls = Cell::new(0);
        let error = input
            .png_bytes_with_cancel(|| {
                calls.set(calls.get() + 1);
                calls.get() >= cancel_at
            })
            .unwrap_err();
        assert!(
            matches!(error, CaptureError::Canceled),
            "callback boundary {cancel_at} returned {error:?}"
        );
    }
    assert_eq!(input.pixels, original);
}

#[test]
fn narrow_blur_shares_the_aggregate_counter_and_keeps_alpha_kernel_pixels() {
    let input = frame(source(1, 65536));
    let original = input.pixels.clone();
    let calls = Cell::new(0);
    let all = crate::PixelRegion {
        x: 0,
        y: 0,
        width: 1,
        height: 65536,
    };
    let result = crate::render_ordered_snapshot_with_cancel(
        &input,
        &[],
        None,
        &[],
        &[],
        None,
        &[all],
        || {
            calls.set(calls.get() + 1);
            false
        },
    )
    .unwrap();
    assert!(
        calls.get() < 256,
        "narrow blur made {} raw token checks",
        calls.get()
    );
    for y in [0u32, 8, 1024, 65535] {
        let mut sums = [0u64; 4];
        let mut count = 0;
        for row in y.saturating_sub(8)..=(y + 8).min(65535) {
            let pixel = input.pixels.get_pixel(0, row).0;
            for channel in 0..3 {
                sums[channel] += u64::from(pixel[channel]) * u64::from(pixel[3]);
            }
            sums[3] += u64::from(pixel[3]);
            count += 1;
        }
        let mut expected = [0u8; 4];
        for channel in 0..3 {
            expected[channel] = sums[channel].checked_div(sums[3]).unwrap_or(0) as u8;
        }
        expected[3] = (sums[3] / count) as u8;
        assert_eq!(result.pixels.get_pixel(0, y).0, expected);
    }
    assert_eq!(input.pixels, original);
}

#[test]
fn sparse_pixelation_region_traversal_is_cancelable_without_scanning_every_crop_row() {
    let pixels = source(1, 65536);
    let original = pixels.clone();
    let grid = crate::PixelationGrid::prepare(&pixels, || false).unwrap();
    let mut output = pixels.clone();
    let regions = (0..100)
        .map(|index| crate::PixelRegion {
            x: 0,
            y: index * 650,
            width: 1,
            height: 1,
        })
        .collect::<Vec<_>>();
    let calls = Cell::new(0);
    let canceled = || {
        calls.set(calls.get() + 1);
        calls.get() >= 3
    };
    let mut work = crate::image_cancel::ImageWork::new(&canceled).unwrap();
    let error = grid
        .paint(
            &mut output,
            crate::PixelRegion {
                x: 0,
                y: 0,
                width: 1,
                height: 65536,
            },
            &regions,
            [1, 65536],
            &mut work,
        )
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<CaptureError>(),
        Some(CaptureError::Canceled)
    ));
    assert_eq!(calls.get(), 3);
    assert_eq!(pixels, original);
}

#[test]
fn empty_horizontal_intersections_preserve_pixels_without_unchecked_height_loops() {
    let input = frame(source(2, 65536));
    let crop = crate::PixelRegion {
        x: 0,
        y: 0,
        width: 1,
        height: 65536,
    };
    let outside = crate::PixelRegion {
        x: 1,
        y: 0,
        width: 1,
        height: 65536,
    };
    let expected = image::imageops::crop_imm(input.pixels.as_ref(), 0, 0, 1, 65536).to_image();
    for annotation in [
        crate::DrawAnnotation::Text(crate::TextAnnotation {
            area: outside,
            color: 0xff0000,
            width: 1,
            height: 1,
            mask: Arc::from([255]),
        }),
        crate::DrawAnnotation::Frame(crate::FrameAnnotation {
            area: outside,
            color: 0x00ff00,
            width: 1,
            filled: true,
        }),
        crate::DrawAnnotation::Highlight(crate::HighlightAnnotation {
            area: outside,
            color: 0x0000ff,
            opacity: 128,
            outside: false,
        }),
    ] {
        let calls = Cell::new(0);
        let result = crate::render_ordered_snapshot_with_cancel(
            &input,
            &[outside],
            Some(crop),
            &[annotation],
            &[],
            None,
            &[],
            || {
                calls.set(calls.get() + 1);
                false
            },
        )
        .unwrap();
        assert_eq!(*result.pixels, expected);
        assert!(calls.get() < 16);
    }
    let no_change = flip_rgba_with_cancel(source(1, 65536), true, || false).unwrap();
    assert_eq!(no_change, source(1, 65536));
}

#[test]
fn vector_cancellation_is_checked_after_rasterization_before_any_layer_flush() {
    let calls = Cell::new(0);
    let canceled = || {
        calls.set(calls.get() + 1);
        calls.get() >= 4
    };
    let mut work = crate::image_cancel::ImageWork::new(&canceled).unwrap();
    work.pixel().unwrap();
    let mut pixels = source(8, 8);
    let original = pixels.clone();
    let area = crate::PixelRegion {
        x: 0,
        y: 0,
        width: 8,
        height: 8,
    };
    let shape = crate::DrawAnnotation::Ellipse(crate::FrameAnnotation {
        area: crate::PixelRegion {
            x: 1,
            y: 1,
            width: 4,
            height: 4,
        },
        color: 0xff0000,
        width: 1,
        filled: true,
    });
    let error =
        crate::layers::paint_ordered(&mut pixels, area, &[shape], [8, 8], &mut work).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<CaptureError>(),
        Some(CaptureError::Canceled)
    ));
    assert_eq!(calls.get(), 4);
    assert_eq!(pixels, original);
}

#[test]
#[ignore = "targeted optimized aggregate-work comparison; synthetic pixels, no desktop or output"]
fn aggregate_work_development_costs() {
    use sha2_10::{Digest, Sha256};
    use std::{
        hint::black_box,
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };
    let flag = AtomicBool::new(false);
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut cases = Vec::new();
    for (name, width, height) in [
        ("narrow-blur", 1, 65536),
        ("wide-blur", 4096, 64),
        ("sparse-pixelation", 1, 262144),
        ("empty-redaction", 2, 262144),
    ] {
        let input = frame(source(width, height));
        let grid = (name == "sparse-pixelation")
            .then(|| crate::PixelationGrid::prepare(&input.pixels, || false).unwrap());
        let mut samples = Vec::new();
        let mut expected = None;
        for sample in 0..35 {
            let all = crate::PixelRegion {
                x: 0,
                y: 0,
                width,
                height,
            };
            let sparse = crate::PixelRegion {
                x: 0,
                y: height - 1,
                width: 1,
                height: 1,
            };
            let outside = crate::PixelRegion {
                x: 1,
                y: 0,
                width: 1,
                height,
            };
            let crop =
                (name == "empty-redaction").then_some(crate::PixelRegion { width: 1, ..all });
            let started = Instant::now();
            let result = crate::render_ordered_snapshot_with_cancel(
                &input,
                if name == "empty-redaction" {
                    std::slice::from_ref(&outside)
                } else {
                    &[]
                },
                crop,
                &[],
                if grid.is_some() {
                    std::slice::from_ref(&sparse)
                } else {
                    &[]
                },
                grid.as_ref(),
                if name.ends_with("blur") {
                    std::slice::from_ref(&all)
                } else {
                    &[]
                },
                || flag.load(Ordering::Relaxed) || Instant::now() >= deadline,
            )
            .unwrap();
            black_box(&result);
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            let digest = format!("{:x}", Sha256::digest(result.pixels.as_raw()));
            if let Some(expected) = &expected {
                assert_eq!(expected, &digest);
            } else {
                expected = Some(digest);
            }
            if sample >= 5 {
                samples.push(elapsed);
            }
        }
        let mut sorted = samples.clone();
        sorted.sort_by(f64::total_cmp);
        cases.push(serde_json::json!({"name":name,"width":width,"height":height,"median_ms":(sorted[14]+sorted[15])/2.0,"p95_ms":sorted[28],"pixels_sha256":expected.unwrap(),"samples_ms":samples}));
    }
    println!(
        "WORK_BUDGET_COST {}",
        serde_json::json!({"cases":cases,"scope":"warm optimized CPU synthetic raster work with an atomic+clock operation callback; no codecs/private memory/presentation/reference-hardware claim"})
    );
}

#[test]
fn unchanged_size_reduction_checks_cancellation_after_its_copy() {
    let pixels = source(1, 1);
    let calls = Cell::new(0);
    assert!(matches!(
        crate::downscale_export_with_cancel(&pixels, crate::ExportSize::Original, || {
            calls.set(calls.get() + 1);
            calls.get() >= 3
        }),
        Err(CaptureError::Canceled)
    ));
    assert_eq!(calls.get(), 3);
}
