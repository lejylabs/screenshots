use crate::PixelRegion;
use anyhow::{Result, ensure};

/// Immutable source-aligned 16-pixel blocks, shared by preview and export.
pub struct PixelationGrid {
    width: u32,
    height: u32,
    columns: u32,
    colors: Vec<[u8; 4]>,
}
impl PixelationGrid {
    pub fn prepare(source: &image::RgbaImage, cancelled: impl Fn() -> bool) -> Result<Self> {
        let (width, height) = source.dimensions();
        crate::validate_image_size(width, height)?;
        let columns = width.div_ceil(16);
        let mut colors =
            Vec::with_capacity((u64::from(columns) * u64::from(height.div_ceil(16))) as usize);
        for y in (0..height).step_by(16) {
            ensure!(!cancelled(), "Pixelation preparation canceled");
            for x in (0..width).step_by(16) {
                ensure!(!cancelled(), "Pixelation preparation canceled");
                let mut sums = [0u64; 4];
                let mut count = 0u64;
                for row in y..(y + 16).min(height) {
                    for column in x..(x + 16).min(width) {
                        let pixel = source.get_pixel(column, row).0;
                        for channel in 0..3 {
                            sums[channel] += u64::from(pixel[channel]) * u64::from(pixel[3]);
                        }
                        sums[3] += u64::from(pixel[3]);
                        count += 1;
                    }
                }
                colors.push([
                    if sums[3] > 0 {
                        (sums[0] / sums[3]) as u8
                    } else {
                        0
                    },
                    if sums[3] > 0 {
                        (sums[1] / sums[3]) as u8
                    } else {
                        0
                    },
                    if sums[3] > 0 {
                        (sums[2] / sums[3]) as u8
                    } else {
                        0
                    },
                    (sums[3] / count) as u8,
                ]);
            }
        }
        Ok(Self {
            width,
            height,
            columns,
            colors,
        })
    }
    /// At most 2048² BGRA pixels (16 MiB); no full-source-sized preview copy.
    pub fn preview_bgra(&self, cancelled: impl Fn() -> bool) -> Result<image::RgbaImage> {
        let scale = (2048.0 / self.width.max(self.height) as f64).min(1.0);
        let width = (self.width as f64 * scale).round().max(1.0) as u32;
        let height = (self.height as f64 * scale).round().max(1.0) as u32;
        let mut preview = image::RgbaImage::new(width, height);
        for y in 0..height {
            ensure!(!cancelled(), "Pixelation preview canceled");
            for x in 0..width {
                let mut color = self.color(
                    (u64::from(x) * u64::from(self.width) / u64::from(width)) as u32,
                    (u64::from(y) * u64::from(self.height) / u64::from(height)) as u32,
                );
                color.swap(0, 2);
                preview.put_pixel(x, y, image::Rgba(color));
            }
        }
        Ok(preview)
    }
    fn color(&self, x: u32, y: u32) -> [u8; 4] {
        self.colors[((y / 16) * self.columns + x / 16) as usize]
    }
    pub(crate) fn paint(
        &self,
        pixels: &mut image::RgbaImage,
        crop: PixelRegion,
        regions: &[PixelRegion],
        source_size: [u32; 2],
    ) -> Result<()> {
        ensure!(
            source_size == [self.width, self.height],
            "Pixelation source dimensions changed"
        );
        let mut spans = Vec::with_capacity(regions.len());
        for y in crop.y..crop.y + crop.height {
            spans.clear();
            for region in regions {
                if y >= region.y && y < region.y + region.height {
                    let start = region.x.max(crop.x);
                    let end = (region.x + region.width).min(crop.x + crop.width);
                    if start < end {
                        spans.push((start, end));
                    }
                }
            }
            spans.sort_unstable();
            let mut painted_until = crop.x;
            for &(start, end) in &spans {
                for x in start.max(painted_until)..end {
                    pixels.put_pixel(x - crop.x, y - crop.y, image::Rgba(self.color(x, y)));
                }
                painted_until = painted_until.max(end);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(pixels: image::RgbaImage) -> crate::CapturedImage {
        crate::CapturedImage {
            display_id: "fixture".into(),
            window_id: None,
            width: pixels.width(),
            height: pixels.height(),
            pixels: std::sync::Arc::new(pixels),
            png_path: "never-written.png".into(),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
        }
    }
    #[test]
    fn preview_and_export_share_source_blocks_alpha_and_crop() -> Result<()> {
        let original = image::RgbaImage::from_fn(33, 17, |x, y| {
            image::Rgba([
                x as u8 * 3,
                y as u8 * 7,
                90,
                if x % 2 == 0 { 0 } else { 255 },
            ])
        });
        let source = source(original.clone());
        let grid = PixelationGrid::prepare(&source.pixels, || false)?;
        let preview = grid.preview_bgra(|| false)?;
        let crop = PixelRegion {
            x: 7,
            y: 3,
            width: 24,
            height: 12,
        };
        let effect = PixelRegion {
            x: 9,
            y: 5,
            width: 20,
            height: 8,
        };
        let mark = crate::FrameAnnotation {
            area: effect,
            color: 0x22c55e,
        };
        let output = crate::render_pixelated_snapshot(
            &source,
            &[],
            Some(crop),
            &[],
            &[],
            &[],
            &[effect, effect],
            Some(&grid),
        )?;
        for y in 0..crop.height {
            for x in 0..crop.width {
                let (sx, sy) = (x + crop.x, y + crop.y);
                let inside = sx >= effect.x
                    && sx < effect.x + effect.width
                    && sy >= effect.y
                    && sy < effect.y + effect.height;
                let expected = if inside {
                    grid.color(sx, sy)
                } else {
                    original.get_pixel(sx, sy).0
                };
                assert_eq!(output.pixels.get_pixel(x, y).0, expected);
                let mut bgra = grid.color(sx, sy);
                bgra.swap(0, 2);
                assert_eq!(preview.get_pixel(sx, sy).0, bgra);
            }
        }
        assert_eq!(grid.color(0, 0), [24, 52, 90, 127]);
        let layered = crate::render_pixelated_snapshot(
            &source,
            &[effect],
            Some(crop),
            &[mark],
            &[],
            &[],
            &[effect],
            Some(&grid),
        )?;
        assert_eq!(layered.pixels.get_pixel(2, 2).0, [0, 0, 0, 255]);
        assert_eq!(source.pixels.as_ref(), &original);
        assert!(
            crate::render_pixelated_snapshot(&source, &[], None, &[], &[], &[], &[effect], None)
                .is_err()
        );
        assert!(PixelationGrid::prepare(&source.pixels, || true).is_err());
        assert!(grid.preview_bgra(|| true).is_err());
        assert_eq!(grid.color(32, 16), [0, 0, 0, 0]);
        let checks = std::cell::Cell::new(0);
        assert!(
            PixelationGrid::prepare(&source.pixels, || {
                checks.set(checks.get() + 1);
                checks.get() > 2
            })
            .is_err()
        );
        assert!(
            crate::render_pixelated_snapshot(
                &source,
                &[],
                None,
                &[],
                &[],
                &[],
                &vec![effect; 101],
                Some(&grid)
            )
            .is_err()
        );
        assert!(
            crate::render_pixelated_snapshot(
                &source,
                &[],
                None,
                &[],
                &[],
                &[],
                &[PixelRegion {
                    x: 32,
                    y: 0,
                    width: 2,
                    height: 1
                }],
                Some(&grid)
            )
            .is_err()
        );
        Ok(())
    }
    #[test]
    #[ignore = "short optimized 4K pixelation timing; no desktop, files or clipboard"]
    fn pixelation_4k_cost() -> Result<()> {
        let source = source(image::RgbaImage::from_pixel(
            3840,
            2160,
            image::Rgba([30, 80, 120, 255]),
        ));
        let start = std::time::Instant::now();
        let grid = PixelationGrid::prepare(&source.pixels, || false)?;
        let prepare = start.elapsed();
        let start = std::time::Instant::now();
        let preview = grid.preview_bgra(|| false)?;
        let preview_time = start.elapsed();
        assert!(preview.as_raw().len() <= 16 * 1024 * 1024);
        let area = PixelRegion {
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
        };
        let start = std::time::Instant::now();
        let output = crate::render_pixelated_snapshot(
            &source,
            &[],
            None,
            &[],
            &[],
            &[],
            &vec![area; 100],
            Some(&grid),
        )?;
        std::hint::black_box(output);
        eprintln!(
            "4K pixelation prepare={prepare:?}, preview={preview_time:?}, 100 overlapping regions export={:?}; grid={} bytes, preview={} bytes",
            start.elapsed(),
            grid.colors.len() * 4,
            preview.as_raw().len()
        );
        Ok(())
    }
}
