use crate::PixelRegion;
use anyhow::{Result, ensure};

const RADIUS: u32 = 8;
const ROWS: usize = (RADIUS * 2 + 1) as usize;
const TILE_WIDTH: u32 = 512;

fn horizontal(source: &image::RgbaImage, y: u32, left: u32, output: &mut [[u64; 4]]) {
    let mut sum = [0u64; 4];
    let sample = |x: u32| {
        let pixel = source.get_pixel(x, y).0;
        [
            u64::from(pixel[0]) * u64::from(pixel[3]),
            u64::from(pixel[1]) * u64::from(pixel[3]),
            u64::from(pixel[2]) * u64::from(pixel[3]),
            u64::from(pixel[3]),
        ]
    };
    for x in left.saturating_sub(RADIUS)..=(left + RADIUS).min(source.width() - 1) {
        let pixel = sample(x);
        for channel in 0..4 {
            sum[channel] += pixel[channel];
        }
    }
    for (offset, value) in output.iter_mut().enumerate() {
        *value = sum;
        let x = left + offset as u32;
        if x >= RADIUS {
            let pixel = sample(x - RADIUS);
            for channel in 0..4 {
                sum[channel] -= pixel[channel];
            }
        }
        if x + RADIUS + 1 < source.width() {
            let pixel = sample(x + RADIUS + 1);
            for channel in 0..4 {
                sum[channel] += pixel[channel];
            }
        }
    }
}

/// Source-aligned, alpha-weighted 17x17 box blur with less than 300 KiB scratch.
fn visit_blurred(
    source: &image::RgbaImage,
    area: PixelRegion,
    mut write: impl FnMut(u32, u32, [u8; 4]),
    cancelled: impl Fn() -> bool,
) -> Result<()> {
    let mut ring = vec![[0u64; 4]; ROWS * TILE_WIDTH as usize];
    let mut sums = vec![[0u64; 4]; TILE_WIDTH as usize];
    for left in (area.x..area.x + area.width).step_by(TILE_WIDTH as usize) {
        ensure!(!cancelled(), "Blur preparation canceled");
        let width = (area.x + area.width - left).min(TILE_WIDTH) as usize;
        sums[..width].fill([0; 4]);
        for row in area.y.saturating_sub(RADIUS)..=(area.y + RADIUS).min(source.height() - 1) {
            let buffer = &mut ring[(row as usize % ROWS) * TILE_WIDTH as usize..][..width];
            horizontal(source, row, left, buffer);
            for (sum, pixel) in sums[..width].iter_mut().zip(buffer.iter()) {
                for channel in 0..4 {
                    sum[channel] += pixel[channel];
                }
            }
        }
        for y in area.y..area.y + area.height {
            ensure!(!cancelled(), "Blur preparation canceled");
            let rows = (y + RADIUS).min(source.height() - 1) - y.saturating_sub(RADIUS) + 1;
            for (offset, sum) in sums[..width].iter().enumerate() {
                let x = left + offset as u32;
                let columns = (x + RADIUS).min(source.width() - 1) - x.saturating_sub(RADIUS) + 1;
                write(
                    x,
                    y,
                    [
                        if sum[3] > 0 {
                            (sum[0] / sum[3]) as u8
                        } else {
                            0
                        },
                        if sum[3] > 0 {
                            (sum[1] / sum[3]) as u8
                        } else {
                            0
                        },
                        if sum[3] > 0 {
                            (sum[2] / sum[3]) as u8
                        } else {
                            0
                        },
                        (sum[3] / u64::from(rows * columns)) as u8,
                    ],
                );
            }
            if y + 1 == area.y + area.height {
                break;
            }
            if y >= RADIUS {
                let buffer = &ring[((y - RADIUS) as usize % ROWS) * TILE_WIDTH as usize..][..width];
                for (sum, pixel) in sums[..width].iter_mut().zip(buffer.iter()) {
                    for channel in 0..4 {
                        sum[channel] -= pixel[channel];
                    }
                }
            }
            if y + RADIUS + 1 < source.height() {
                let row = y + RADIUS + 1;
                let buffer = &mut ring[(row as usize % ROWS) * TILE_WIDTH as usize..][..width];
                horizontal(source, row, left, buffer);
                for (sum, pixel) in sums[..width].iter_mut().zip(buffer.iter()) {
                    for channel in 0..4 {
                        sum[channel] += pixel[channel];
                    }
                }
            }
        }
    }
    Ok(())
}

/// At most 1024² BGRA pixels (4 MiB), sampled from the full-resolution blur.
pub fn blur_preview_bgra(
    source: &image::RgbaImage,
    cancelled: impl Fn() -> bool,
) -> Result<image::RgbaImage> {
    crate::validate_image_size(source.width(), source.height())?;
    ensure!(!cancelled(), "Blur preparation canceled");
    let scale = (1024.0 / source.width().max(source.height()) as f64).min(1.0);
    let width = (source.width() as f64 * scale).round().max(1.0) as u32;
    let height = (source.height() as f64 * scale).round().max(1.0) as u32;
    let mut preview = image::RgbaImage::new(width, height);
    visit_blurred(
        source,
        PixelRegion {
            x: 0,
            y: 0,
            width: source.width(),
            height: source.height(),
        },
        |x, y, mut color| {
            let px = ((u64::from(x + 1) * u64::from(width) - 1) / u64::from(source.width())) as u32;
            let py =
                ((u64::from(y + 1) * u64::from(height) - 1) / u64::from(source.height())) as u32;
            if u64::from(px) * u64::from(source.width()) / u64::from(width) != u64::from(x)
                || u64::from(py) * u64::from(source.height()) / u64::from(height) != u64::from(y)
            {
                return;
            }
            color.swap(0, 2);
            preview.put_pixel(px, py, image::Rgba(color));
        },
        cancelled,
    )?;
    Ok(preview)
}

pub(crate) fn paint_blur(
    source: &image::RgbaImage,
    output: &mut image::RgbaImage,
    crop: PixelRegion,
    regions: &[PixelRegion],
) -> Result<()> {
    // Scan the union in horizontal bands so overlapping regions never multiply image work.
    let mut edges = vec![crop.y, crop.y + crop.height];
    for region in regions {
        edges.push(region.y.clamp(crop.y, crop.y + crop.height));
        edges.push((region.y + region.height).clamp(crop.y, crop.y + crop.height));
    }
    edges.sort_unstable();
    edges.dedup();
    let mut spans = Vec::with_capacity(regions.len());
    for band in edges.windows(2) {
        spans.clear();
        for region in regions {
            if band[0] >= region.y && band[0] < region.y + region.height {
                let start = region.x.max(crop.x);
                let end = (region.x + region.width).min(crop.x + crop.width);
                if start < end {
                    spans.push((start, end));
                }
            }
        }
        spans.sort_unstable();
        let mut index = 0;
        while index < spans.len() {
            let (start, mut end) = spans[index];
            index += 1;
            while index < spans.len() && spans[index].0 <= end {
                end = end.max(spans[index].1);
                index += 1;
            }
            visit_blurred(
                source,
                PixelRegion {
                    x: start,
                    y: band[0],
                    width: end - start,
                    height: band[1] - band[0],
                },
                |x, y, color| output.put_pixel(x - crop.x, y - crop.y, image::Rgba(color)),
                || false,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reference(source: &image::RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let mut sum = [0u64; 4];
        let mut count = 0;
        for row in y.saturating_sub(RADIUS)..=(y + RADIUS).min(source.height() - 1) {
            for column in x.saturating_sub(RADIUS)..=(x + RADIUS).min(source.width() - 1) {
                let p = source.get_pixel(column, row).0;
                for channel in 0..3 {
                    sum[channel] += u64::from(p[channel]) * u64::from(p[3]);
                }
                sum[3] += u64::from(p[3]);
                count += 1;
            }
        }
        [
            if sum[3] > 0 {
                (sum[0] / sum[3]) as u8
            } else {
                0
            },
            if sum[3] > 0 {
                (sum[1] / sum[3]) as u8
            } else {
                0
            },
            if sum[3] > 0 {
                (sum[2] / sum[3]) as u8
            } else {
                0
            },
            (sum[3] / count) as u8,
        ]
    }
    fn fixture(width: u32, height: u32) -> crate::CapturedImage {
        crate::CapturedImage {
            display_id: "fixture".into(),
            window_id: None,
            width,
            height,
            pixels: std::sync::Arc::new(image::RgbaImage::from_fn(width, height, |x, y| {
                image::Rgba([
                    (x * 17) as u8,
                    (y * 33) as u8,
                    90,
                    if (x + y) % 3 == 0 { 0 } else { 255 },
                ])
            })),
            png_path: "never-written.png".into(),
            elapsed_ms: 0,
            source_format: crate::PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        }
    }
    #[test]
    fn streaming_blur_matches_reference_at_edges_alpha_and_tile_boundaries() -> Result<()> {
        for (width, height) in [(531, 25), (1, 19), (19, 1), (1, 1)] {
            let source = fixture(width, height);
            let original = source.pixels.clone();
            let area = PixelRegion {
                x: 0,
                y: 0,
                width,
                height,
            };
            let output = crate::render_filtered_snapshot(
                &source,
                &[],
                None,
                &[],
                &[],
                &[],
                &[],
                None,
                &[area, area],
            )?;
            for y in 0..height {
                for x in 0..width {
                    assert_eq!(
                        output.pixels.get_pixel(x, y).0,
                        reference(&source.pixels, x, y),
                        "at {x},{y} in {width}x{height}"
                    );
                }
            }
            assert!(std::sync::Arc::ptr_eq(&source.pixels, &original));
        }
        Ok(())
    }
    #[test]
    fn preview_sampling_crop_and_redaction_use_original_source() -> Result<()> {
        let source = fixture(1031, 19);
        let preview = blur_preview_bgra(&source.pixels, || false)?;
        assert_eq!(preview.width(), 1024);
        for y in 0..preview.height() {
            for x in 0..preview.width() {
                let sx = (u64::from(x) * 1031 / u64::from(preview.width())) as u32;
                let sy = (u64::from(y) * 19 / u64::from(preview.height())) as u32;
                let mut expected = reference(&source.pixels, sx, sy);
                expected.swap(0, 2);
                assert_eq!(preview.get_pixel(x, y).0, expected);
            }
        }
        let crop = PixelRegion {
            x: 505,
            y: 4,
            width: 20,
            height: 9,
        };
        let effect = PixelRegion {
            x: 510,
            y: 0,
            width: 18,
            height: 19,
        };
        let output = crate::render_filtered_snapshot(
            &source,
            &[],
            Some(crop),
            &[],
            &[],
            &[],
            &[],
            None,
            &[effect, effect],
        )?;
        for y in 0..crop.height {
            for x in 0..crop.width {
                let sx = x + crop.x;
                let sy = y + crop.y;
                let expected = if sx >= 510 {
                    reference(&source.pixels, sx, sy)
                } else {
                    source.pixels.get_pixel(sx, sy).0
                };
                assert_eq!(output.pixels.get_pixel(x, y).0, expected);
            }
        }
        let grid = crate::PixelationGrid::prepare(&source.pixels, || false)?;
        let pixelated = crate::render_filtered_snapshot(
            &source,
            &[],
            Some(crop),
            &[],
            &[],
            &[],
            &[effect],
            Some(&grid),
            &[effect],
        )?;
        let expected = crate::render_pixelated_snapshot(
            &source,
            &[],
            Some(crop),
            &[],
            &[],
            &[],
            &[effect],
            Some(&grid),
        )?;
        assert_eq!(pixelated.pixels, expected.pixels);
        let redacted = crate::render_filtered_snapshot(
            &source,
            &[effect],
            Some(crop),
            &[],
            &[],
            &[],
            &[],
            None,
            &[effect],
        )?;
        assert_eq!(redacted.pixels.get_pixel(5, 0).0, [0, 0, 0, 255]);
        assert!(blur_preview_bgra(&source.pixels, || true).is_err());
        assert!(
            crate::render_filtered_snapshot(
                &source,
                &[],
                None,
                &[],
                &[],
                &[],
                &[],
                None,
                &vec![effect; 101]
            )
            .is_err()
        );
        Ok(())
    }
    #[test]
    #[ignore = "short optimized 4K blur timing; no desktop, files or clipboard"]
    fn blur_4k_cost() -> Result<()> {
        let source = fixture(3840, 2160);
        let start = std::time::Instant::now();
        let preview = blur_preview_bgra(&source.pixels, || false)?;
        let preview_time = start.elapsed();
        let area = PixelRegion {
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
        };
        let start = std::time::Instant::now();
        let output = crate::render_filtered_snapshot(
            &source,
            &[],
            None,
            &[],
            &[],
            &[],
            &[],
            None,
            &vec![area; 100],
        )?;
        std::hint::black_box(output);
        eprintln!(
            "4K blur preview={preview_time:?}, 100 overlapping regions export={:?}; preview={} bytes, scratch={} bytes",
            start.elapsed(),
            preview.as_raw().len(),
            (ROWS + 1) * TILE_WIDTH as usize * std::mem::size_of::<[u64; 4]>()
        );
        Ok(())
    }
}
