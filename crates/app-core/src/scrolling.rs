//! Bounded vertical assembly of successive live scrolling viewports.

use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use std::{io::Write, path::Path};

const MAX_VIEWPORT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_COMPARE_BYTES: usize = 512 * 1024 * 1024;
const MAX_APPENDS: usize = 128;

#[derive(Clone, Copy, Debug)]
pub struct ScrollMatchOptions {
    pub minimum_overlap: u32,
    pub tolerance: u8,
    pub trim_top: u32,
    pub trim_bottom: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollAppend {
    Unchanged,
    Advanced(u32),
}

pub struct ScrollAssembler {
    previous: RgbaImage,
    output: Vec<u8>,
    output_height: u32,
    options: ScrollMatchOptions,
    remaining: usize,
    advances: Vec<u32>,
}

fn viewport(image: RgbaImage, options: ScrollMatchOptions) -> Result<RgbaImage> {
    let (width, height) = image.dimensions();
    crate::validate_image_size(width, height)?;
    ensure!(
        width <= 4096
            && height <= 4096
            && u64::from(width) * u64::from(height) * 4 <= MAX_VIEWPORT_BYTES
            && image.as_raw().len() as u64 == u64::from(width) * u64::from(height) * 4,
        "Scrolling viewport exceeds its dimensions or pixel budget"
    );
    let trim = options
        .trim_top
        .checked_add(options.trim_bottom)
        .context("Scrolling trim overflow")?;
    let height = height
        .checked_sub(trim)
        .filter(|height| *height >= 2)
        .context("Scrolling trim must leave at least two rows")?;
    ensure!(
        options.minimum_overlap > 0
            && options.minimum_overlap <= 1024
            && options.minimum_overlap < height
            && options.tolerance <= 16,
        "Invalid scrolling match options"
    );
    if trim == 0 {
        return Ok(image);
    }
    let stride = width as usize * 4;
    let mut bytes = image.into_raw();
    let first = options.trim_top as usize * stride;
    let end = first + height as usize * stride;
    bytes.copy_within(first..end, 0);
    bytes.truncate(height as usize * stride);
    RgbaImage::from_raw(width, height, bytes).context("Invalid trimmed scrolling pixels")
}

fn close(
    left: &[u8],
    right: &[u8],
    tolerance: u8,
    remaining: &mut usize,
    cancelled: &impl Fn() -> bool,
) -> Result<bool> {
    ensure!(left.len() == right.len(), "Invalid scrolling comparison");
    for (index, (left, right)) in left.iter().zip(right).enumerate() {
        if index % 4096 == 0 {
            ensure!(!cancelled(), "Scrolling capture canceled");
        }
        *remaining = remaining
            .checked_sub(1)
            .context("Scrolling matching exceeds its 512 MiB work budget")?;
        if left.abs_diff(*right) > tolerance {
            return Ok(false);
        }
    }
    Ok(true)
}

impl ScrollAssembler {
    /// Encode once and recheck cancellation before atomic PNG publication.
    pub fn save_png(
        image: &RgbaImage,
        destination: &Path,
        cancelled: impl Fn() -> bool,
    ) -> Result<()> {
        use image::ImageEncoder;
        crate::validate_image_size(image.width(), image.height())?;
        ensure!(
            image.as_raw().len() as u64 == u64::from(image.width()) * u64::from(image.height()) * 4,
            "Scrolling output backing does not match its dimensions"
        );
        let check = || {
            if cancelled() {
                Err(crate::CaptureError::Canceled)
            } else {
                Ok(())
            }
        };
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
                    image.as_raw(),
                    image.width(),
                    image.height(),
                    image::ExtendedColorType::Rgba8,
                )?;
                writer.flush()?;
                Ok(())
            },
            check,
        )?;
        Ok(())
    }
    pub fn new(first: RgbaImage, options: ScrollMatchOptions) -> Result<Self> {
        let first = viewport(first, options)?;
        let mut output = Vec::new();
        output
            .try_reserve_exact(first.as_raw().len())
            .context("Could not allocate scrolling output")?;
        output.extend_from_slice(first.as_raw());
        let height = first.height();
        Ok(Self {
            previous: first,
            output,
            output_height: height,
            options,
            remaining: MAX_COMPARE_BYTES,
            advances: Vec::new(),
        })
    }

    pub fn advances(&self) -> &[u32] {
        &self.advances
    }

    pub fn push(&mut self, next: RgbaImage, cancelled: impl Fn() -> bool) -> Result<ScrollAppend> {
        ensure!(!cancelled(), "Scrolling capture canceled");
        let next = viewport(next, self.options)?;
        ensure!(
            next.dimensions() == self.previous.dimensions(),
            "Scrolling viewport dimensions changed"
        );
        ensure!(
            self.advances.len() < MAX_APPENDS,
            "Scrolling append limit exceeded"
        );
        if close(
            self.previous.as_raw(),
            next.as_raw(),
            0,
            &mut self.remaining,
            &cancelled,
        )? {
            return Ok(ScrollAppend::Unchanged);
        }
        let width = next.width() as usize;
        let height = next.height();
        let stride = width * 4;
        let mut matched = None;
        for overlap in self.options.minimum_overlap..height {
            ensure!(!cancelled(), "Scrolling capture canceled");
            let advance = height - overlap;
            let rows = [0, overlap / 2, overlap - 1];
            let mut candidate = true;
            'sample: for y in rows {
                for column in 0..16usize {
                    let x = column * (width - 1) / 15;
                    let old = ((advance + y) as usize * width + x) * 4;
                    let new = (y as usize * width + x) * 4;
                    if !close(
                        &self.previous.as_raw()[old..old + 4],
                        &next.as_raw()[new..new + 4],
                        self.options.tolerance,
                        &mut self.remaining,
                        &cancelled,
                    )? {
                        candidate = false;
                        break 'sample;
                    }
                }
            }
            if candidate
                && close(
                    &self.previous.as_raw()[advance as usize * stride..],
                    &next.as_raw()[..overlap as usize * stride],
                    self.options.tolerance,
                    &mut self.remaining,
                    &cancelled,
                )?
            {
                ensure!(
                    matched.is_none(),
                    "Scrolling overlap is ambiguous; crop to distinctive content or reduce tolerance"
                );
                matched = Some(overlap);
            }
        }
        let overlap = matched.context("Scrolling images have no reliable vertical overlap")?;
        let advance = height - overlap;
        let new_height = self
            .output_height
            .checked_add(advance)
            .context("Scrolling output height overflow")?;
        crate::validate_image_size(next.width(), new_height)?;
        let suffix = &next.as_raw()[overlap as usize * stride..];
        self.output
            .try_reserve_exact(suffix.len())
            .context("Could not grow scrolling output")?;
        ensure!(!cancelled(), "Scrolling capture canceled");
        self.output.extend_from_slice(suffix);
        self.output_height = new_height;
        self.advances.push(advance);
        self.previous = next;
        Ok(ScrollAppend::Advanced(advance))
    }

    pub fn finish(self) -> Result<RgbaImage> {
        RgbaImage::from_raw(self.previous.width(), self.output_height, self.output)
            .context("Scrolling output backing does not match its dimensions")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    fn document(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            let v = (x * 17 + y * 991).wrapping_mul(0x85ebca6b);
            Rgba([v as u8, (v >> 8) as u8, (v >> 16) as u8, 255])
        })
    }
    fn options() -> ScrollMatchOptions {
        ScrollMatchOptions {
            minimum_overlap: 4,
            tolerance: 0,
            trim_top: 0,
            trim_bottom: 0,
        }
    }
    #[test]
    fn live_viewports_append_exactly_and_recognize_repeated_end() {
        let source = document(11, 53);
        let frame = |y| image::imageops::crop_imm(&source, 0, y, 11, 20).to_image();
        let mut output = ScrollAssembler::new(frame(0), options()).unwrap();
        assert_eq!(
            output.push(frame(13), || false).unwrap(),
            ScrollAppend::Advanced(13)
        );
        assert_eq!(
            output.push(frame(26), || false).unwrap(),
            ScrollAppend::Advanced(13)
        );
        assert_eq!(
            output.push(frame(33), || false).unwrap(),
            ScrollAppend::Advanced(7)
        );
        assert_eq!(
            output.push(frame(33), || false).unwrap(),
            ScrollAppend::Unchanged
        );
        assert_eq!(output.advances(), [13, 13, 7]);
        assert_eq!(output.finish().unwrap(), source);
    }
    #[test]
    fn fuzzy_overlap_preserves_original_pixels_and_explicit_header_trims() {
        let source = document(9, 35);
        let frame = |y, jitter| {
            let mut output = RgbaImage::from_pixel(9, 24, Rgba([50, 51, 52, 255]));
            for row in 0..20 {
                for x in 0..9 {
                    let mut value = source.get_pixel(x, y + row).0;
                    if jitter {
                        value[0] = value[0].saturating_add(1);
                    }
                    output.put_pixel(x, row + 2, Rgba(value));
                }
            }
            output
        };
        let mut opts = options();
        opts.trim_top = 2;
        opts.trim_bottom = 2;
        opts.tolerance = 1;
        let mut output = ScrollAssembler::new(frame(0, false), opts).unwrap();
        assert_eq!(
            output.push(frame(15, true), || false).unwrap(),
            ScrollAppend::Advanced(15)
        );
        let result = output.finish().unwrap();
        assert_eq!(result.dimensions(), (9, 35));
        assert_eq!(
            image::imageops::crop_imm(&result, 0, 0, 9, 20).to_image(),
            image::imageops::crop_imm(&source, 0, 0, 9, 20).to_image()
        );
        assert_eq!(
            image::imageops::crop_imm(&result, 0, 20, 9, 15).to_image(),
            image::imageops::crop_imm(&frame(15, true), 0, 7, 9, 15).to_image()
        );
    }
    #[test]
    fn invalid_ambiguous_wrong_motion_and_work_limits_fail_without_output_mutation() {
        let source = document(8, 40);
        let frame = |y| image::imageops::crop_imm(&source, 0, y, 8, 20).to_image();
        let mut output = ScrollAssembler::new(frame(10), options()).unwrap();
        assert!(output.push(frame(0), || false).is_err());
        assert!(output.push(frame(20), || true).is_err());
        assert_eq!(output.finish().unwrap(), frame(10));
        let first = RgbaImage::from_pixel(8, 20, Rgba([5, 6, 7, 255]));
        let mut next = first.clone();
        next.put_pixel(0, 19, Rgba([255; 4]));
        let mut output = ScrollAssembler::new(first, options()).unwrap();
        assert!(
            output
                .push(next, || false)
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
        let mut output = ScrollAssembler::new(frame(0), options()).unwrap();
        output.remaining = 0;
        assert!(output.push(frame(10), || false).is_err());
        let mut opts = options();
        opts.trim_top = 19;
        assert!(ScrollAssembler::new(frame(0), opts).is_err());
        assert!(ScrollAssembler::new(RgbaImage::new(0, 1), options()).is_err());
        assert!(
            ScrollAssembler::new(RgbaImage::from_raw(1, 2, vec![0; 9]).unwrap(), options())
                .is_err()
        );
    }

    #[test]
    fn canceled_png_publication_removes_only_its_staging_output() {
        let root = std::env::temp_dir().join(format!(
            "screenshots-scroll-publish-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let image = document(8, 20);
        let output = root.join("scroll.png");
        let checks = std::cell::Cell::new(0);
        let result = ScrollAssembler::save_png(&image, &output, || {
            checks.set(checks.get() + 1);
            checks.get() >= 2
        });
        assert!(result.is_err());
        assert!(!output.exists());
        assert_eq!(root.read_dir().unwrap().count(), 0);
        ScrollAssembler::save_png(&image, &output, || false).unwrap();
        let bytes = std::fs::read(&output).unwrap();
        assert!(ScrollAssembler::save_png(&image, &output, || false).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), bytes);
        assert_eq!(image::open(&output).unwrap().into_rgba8(), image);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "optimized synthetic scrolling overlap throughput; no native input"]
    fn scrolling_matching_cost() {
        let source = document(1920, 1780);
        let first = image::imageops::crop_imm(&source, 0, 0, 1920, 1080).to_image();
        let next = image::imageops::crop_imm(&source, 0, 700, 1920, 1080).to_image();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mut times = Vec::new();
        for sample in 0..12 {
            let mut assembler = ScrollAssembler::new(
                first.clone(),
                ScrollMatchOptions {
                    minimum_overlap: 32,
                    ..options()
                },
            )
            .unwrap();
            let frame = next.clone();
            let start = std::time::Instant::now();
            assert_eq!(
                assembler
                    .push(frame, || cancel.load(std::sync::atomic::Ordering::Relaxed))
                    .unwrap(),
                ScrollAppend::Advanced(700)
            );
            let elapsed = start.elapsed();
            assert_eq!(assembler.finish().unwrap(), source);
            if sample >= 2 {
                times.push(elapsed);
            }
        }
        eprintln!(
            "Scrolling matching: 2 warmups, 10 samples, raw={times:?}; viewport=8294400 bytes; output=13670400 bytes; atomic cancel"
        );
        times.sort_unstable();
        eprintln!(
            "Scrolling overlap median={:?}, p95={:?}; fixture/reference bytes excluded from production budget; not hardware acceptance",
            (times[4] + times[5]) / 2,
            times[9]
        );
    }
}
