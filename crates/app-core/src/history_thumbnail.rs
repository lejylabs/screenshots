//! Cancellable RGBA specialization of image 0.25.10 thumbnail downscaling.
//!
//! Derived from image-rs Developers' `image` 0.25.10 `imageops/sample.rs`
//! (`thumbnail`, `thumbnail_sample_block`) and `math/utils.rs` (`resize_dimensions`).
//! Distributed under the upstream MIT option; the full notice is retained in
//! THIRD_PARTY_NOTICES.md. Review the upstream-oracle tests on image upgrades.

use anyhow::{Result, ensure};
use image::RgbaImage;

const SAMPLE_CHECK_INTERVAL: usize = 4096;

pub(crate) fn history_thumbnail_rgba(
    source: &RgbaImage,
    cancelled: impl Fn() -> bool,
) -> Result<RgbaImage> {
    reduce(
        source,
        &cancelled,
        #[cfg(test)]
        || {},
    )
}

fn reduce(
    source: &RgbaImage,
    cancelled: &impl Fn() -> bool,
    #[cfg(test)] sampled: impl Fn(),
) -> Result<RgbaImage> {
    let (width, height) = source.dimensions();
    crate::validate_image_size(width, height)?;
    ensure!(
        source.as_raw().len() as u64 == u64::from(width) * u64::from(height) * 4,
        "Thumbnail source backing does not match its dimensions"
    );
    ensure!(!cancelled(), "Preview canceled");

    let ratio = (f64::from(width.min(400)) / f64::from(width))
        .min(f64::from(height.min(240)) / f64::from(height));
    let output_width = ((f64::from(width) * ratio).round() as u32).max(1);
    let output_height = ((f64::from(height) * ratio).round() as u32).max(1);
    let mut output = RgbaImage::new(output_width, output_height);
    let x_ratio = width as f32 / output_width as f32;
    let y_ratio = height as f32 / output_height as f32;
    let mut until_check = 0;

    for output_y in 0..output_height {
        ensure!(!cancelled(), "Preview canceled");
        let bottom_f = output_y as f32 * y_ratio;
        let top_f = bottom_f + y_ratio;
        let bottom = (bottom_f.ceil() as u32).clamp(0, height - 1);
        let top = (top_f.ceil() as u32).clamp(bottom, height);
        for output_x in 0..output_width {
            let left_f = output_x as f32 * x_ratio;
            let right_f = left_f + x_ratio;
            let left = (left_f.ceil() as u32).clamp(0, width - 1);
            let right = (right_f.ceil() as u32).clamp(left, width);
            ensure!(
                left < right && bottom < top,
                "Thumbnail sample window is empty"
            );
            let mut sums = [0u64; 4];
            for y in bottom..top {
                let row_start = (y as usize * width as usize + left as usize) * 4;
                let row_end = (y as usize * width as usize + right as usize) * 4;
                let mut row = &source.as_raw()[row_start..row_end];
                while !row.is_empty() {
                    if until_check == 0 {
                        ensure!(!cancelled(), "Preview canceled");
                        until_check = SAMPLE_CHECK_INTERVAL;
                    }
                    let count = (row.len() / 4).min(until_check);
                    let (chunk, rest) = row.split_at(count * 4);
                    for pixel in chunk.chunks_exact(4) {
                        sums[0] += u64::from(pixel[0]);
                        sums[1] += u64::from(pixel[1]);
                        sums[2] += u64::from(pixel[2]);
                        sums[3] += u64::from(pixel[3]);
                        #[cfg(test)]
                        sampled();
                    }
                    until_check -= count;
                    row = rest;
                }
            }
            let count = u64::from(right - left) * u64::from(top - bottom);
            output.put_pixel(
                output_x,
                output_y,
                image::Rgba(sums.map(|sum| ((sum + count / 2) / count) as u8)),
            );
        }
    }
    ensure!(!cancelled(), "Preview canceled");
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgba};
    use std::cell::Cell;

    fn pattern(width: u32, height: u32, seed: &mut u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 17;
            *seed ^= *seed << 5;
            Rgba([
                *seed as u8,
                (*seed >> 8) as u8,
                (*seed >> 16) as u8,
                [0, 128, 255][(x + y) as usize % 3],
            ])
        })
    }

    fn assert_oracle(source: RgbaImage) {
        let original = source.clone();
        let expected = DynamicImage::ImageRgba8(source.clone())
            .thumbnail(source.width().min(400), source.height().min(240))
            .into_rgba8();
        let actual = history_thumbnail_rgba(&source, || false).unwrap();
        assert_eq!(actual, expected, "source {:?}", source.dimensions());
        assert!(actual.width() <= 400 && actual.height() <= 240);
        assert_eq!(source, original);
    }

    #[test]
    fn exact_upstream_oracle_for_small_edges_aspect_ratios_and_seeded_pixels() {
        let mut seed = 0x5448_554du32;
        for width in 1..=32 {
            for height in 1..=32 {
                assert_oracle(pattern(width, height, &mut seed));
            }
        }
        for width in [399, 400, 401] {
            for height in [239, 240, 241] {
                assert_oracle(pattern(width, height, &mut seed));
            }
        }
        for (width, height) in [
            (3840, 2160),
            (240, 400),
            (1921, 1081),
            (10000, 1),
            (1, 10000),
            (8193, 3),
            (3, 8193),
            (1_000_003, 1),
            (1, 1_000_003),
        ] {
            assert_oracle(pattern(width, height, &mut seed));
        }
        let mut seed = 0x5448_554du32;
        for _ in 0..32 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let width = seed % 1024 + 1;
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let height = seed % 1024 + 1;
            assert_oracle(pattern(width, height, &mut seed));
        }
    }

    #[test]
    fn channel_rounding_includes_alpha_and_hidden_rgb() {
        let source = RgbaImage::from_fn(800, 1, |x, _| {
            if x % 2 == 0 {
                Rgba([1, 10, 99, 0])
            } else {
                Rgba([2, 11, 100, 255])
            }
        });
        let result = history_thumbnail_rgba(&source, || false).unwrap();
        assert_eq!(result.dimensions(), (400, 1));
        assert!(result.pixels().all(|pixel| pixel.0 == [2, 11, 100, 128]));
    }

    #[test]
    fn cancellation_is_checked_before_sampling_within_large_samples_and_before_return() {
        let tiny = RgbaImage::from_pixel(1, 1, Rgba([10, 20, 30, 40]));
        let samples = Cell::new(0usize);
        let error = reduce(&tiny, &|| true, || samples.set(samples.get() + 1)).unwrap_err();
        assert_eq!(error.to_string(), "Preview canceled");
        assert_eq!(samples.get(), 0);

        let error = reduce(&tiny, &|| samples.get() == 1, || {
            samples.set(samples.get() + 1)
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "Preview canceled");
        assert_eq!(samples.get(), 1);

        // One output window spans 5000 source pixels: checking only rows or
        // output pixels would miss the flag until after this window finished.
        let source = RgbaImage::from_pixel(2_000_000, 1, Rgba([11, 22, 33, 44]));
        samples.set(0);
        let last_check = Cell::new(0);
        let cancelled = || {
            assert!(samples.get() - last_check.get() <= SAMPLE_CHECK_INTERVAL);
            last_check.set(samples.get());
            samples.get() >= 1
        };
        let result = reduce(&source, &cancelled, || samples.set(samples.get() + 1));
        assert_eq!(result.unwrap_err().to_string(), "Preview canceled");
        assert_eq!(samples.get(), SAMPLE_CHECK_INTERVAL);
        assert!(source.pixels().all(|pixel| pixel.0 == [11, 22, 33, 44]));
    }

    #[test]
    fn malformed_backing_and_size_are_rejected_before_cancellation_or_allocation() {
        for source in [
            RgbaImage::new(0, 1),
            RgbaImage::new(1, 0),
            RgbaImage::from_raw(1, 1, vec![1; 8]).unwrap(),
        ] {
            assert!(
                history_thumbnail_rgba(&source, || panic!("invalid input must fail first"))
                    .is_err()
            );
        }
    }

    #[test]
    fn maximum_admitted_source_has_bounded_checks_and_one_output() {
        let mut bytes = Vec::with_capacity(crate::MAX_CAPTURE_BYTES as usize + 4);
        bytes.resize(crate::MAX_CAPTURE_BYTES as usize, 255);
        let mut source = RgbaImage::from_raw(1 << 26, 1, bytes).unwrap();
        let samples = Cell::new(0usize);
        let last_check = Cell::new(0usize);
        let output = reduce(
            &source,
            &|| {
                assert!(samples.get() - last_check.get() <= SAMPLE_CHECK_INTERVAL);
                last_check.set(samples.get());
                false
            },
            || samples.set(samples.get() + 1),
        )
        .unwrap();
        assert_eq!(output.dimensions(), (400, 1));
        assert!(output.pixels().all(|pixel| pixel.0 == [255; 4]));
        assert!(samples.get() > SAMPLE_CHECK_INTERVAL);
        assert_eq!(last_check.get(), samples.get());
        assert!(source.pixels().all(|pixel| pixel.0 == [255; 4]));
        drop(output);
        bytes = source.into_raw();
        bytes.push(255);
        source = RgbaImage::from_raw(1 << 26, 1, bytes).unwrap();
        assert!(
            history_thumbnail_rgba(&source, || panic!("extra backing must fail first")).is_err()
        );
        let mut bytes = source.into_raw();
        bytes.extend_from_slice(&[255; 3]);
        let source = RgbaImage::from_raw((1 << 26) + 1, 1, bytes).unwrap();
        assert!(history_thumbnail_rgba(&source, || panic!("oversize must fail first")).is_err());
    }

    #[test]
    #[ignore = "optimized synthetic already-decoded 4K comparison; no capture or clipboard"]
    fn history_thumbnail_4k_cost() {
        let source = pattern(3840, 2160, &mut 0x5448_554d);
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut upstream = Vec::new();
        let mut cancellable = Vec::new();
        for sample in 0..12 {
            let start = std::time::Instant::now();
            let expected = std::hint::black_box(image::imageops::thumbnail(&source, 400, 225));
            let old = start.elapsed();
            let start = std::time::Instant::now();
            let actual = std::hint::black_box(
                history_thumbnail_rgba(&source, || {
                    cancel.load(std::sync::atomic::Ordering::Relaxed)
                })
                .unwrap(),
            );
            let new = start.elapsed();
            assert_eq!(actual, expected);
            if sample >= 2 {
                upstream.push(old);
                cancellable.push(new);
            }
        }
        eprintln!(
            "raw upstream={upstream:?}; cancellable={cancellable:?}; cancel=AtomicBool::load(Relaxed)"
        );
        upstream.sort_unstable();
        cancellable.sort_unstable();
        let median = |times: &[std::time::Duration]| (times[4] + times[5]) / 2;
        eprintln!(
            "Already-decoded 4K thumbnail; 2 warmups, 10 interleaved samples: upstream median={:?} p95={:?}; cancellable median={:?} p95={:?}; source=33177600 bytes, each output=360000 bytes. Development observation, not hardware acceptance.",
            median(&upstream),
            upstream[9],
            median(&cancellable),
            cancellable[9]
        );
    }
}
