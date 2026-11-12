use crate::{CaptureError, validate_image_size};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportSize {
    #[default]
    Original,
    Half,
    Quarter,
    Width(u32),
    Height(u32),
    Exact(u32, u32),
}

impl ExportSize {
    pub fn validate(self) -> Result<(), CaptureError> {
        if let Self::Width(value) | Self::Height(value) = self {
            validate_image_size(value, 1)?;
        }
        if let Self::Exact(width, height) = self {
            validate_image_size(width, height)?;
        }
        Ok(())
    }
    pub fn dimensions(self, width: u32, height: u32) -> Result<(u32, u32), CaptureError> {
        validate_image_size(width, height)?;
        self.validate()?;
        let result = match self {
            Self::Original => (width, height),
            Self::Half => (width.div_ceil(2), height.div_ceil(2)),
            Self::Quarter => (width.div_ceil(4), height.div_ceil(4)),
            Self::Exact(target_width, target_height) => {
                if target_width > width || target_height > height {
                    return Err(CaptureError::Backend(
                        "Export dimensions cannot exceed the source dimensions".into(),
                    ));
                }
                (target_width, target_height)
            }
            Self::Width(target) => {
                if target > width {
                    return Err(CaptureError::Backend(
                        "Export width cannot exceed the source width".into(),
                    ));
                }
                (
                    target,
                    (u64::from(height) * u64::from(target)).div_ceil(u64::from(width)) as u32,
                )
            }
            Self::Height(target) => {
                if target > height {
                    return Err(CaptureError::Backend(
                        "Export height cannot exceed the source height".into(),
                    ));
                }
                (
                    (u64::from(width) * u64::from(target)).div_ceil(u64::from(height)) as u32,
                    target,
                )
            }
        };
        validate_image_size(result.0, result.1)?;
        Ok(result)
    }
    pub fn label(self) -> String {
        match self {
            Self::Original => "Original".into(),
            Self::Half => "50%".into(),
            Self::Quarter => "25%".into(),
            Self::Width(width) => format!("Width {width} px"),
            Self::Height(height) => format!("Height {height} px"),
            Self::Exact(width, height) => format!("{width} x {height} px"),
        }
    }
}

/// Downscale finalized RGBA pixels without a full-size float intermediate.
pub(crate) fn downscale_export(
    source: &image::RgbaImage,
    size: ExportSize,
) -> Result<image::RgbaImage, CaptureError> {
    let (width, height) = size.dimensions(source.width(), source.height())?;
    if (width, height) == source.dimensions() {
        return Ok(source.clone());
    }
    let factor = match size {
        ExportSize::Half => Some(2),
        ExportSize::Quarter => Some(4),
        _ => None,
    };
    let mut output = image::RgbaImage::new(width, height);
    for (x, y, pixel) in output.enumerate_pixels_mut() {
        let (left, right, top, bottom) = if let Some(factor) = factor {
            (
                x * factor,
                ((x + 1) * factor).min(source.width()),
                y * factor,
                ((y + 1) * factor).min(source.height()),
            )
        } else {
            let map = |value: u32, input: u32, output: u32| {
                (u64::from(value) * u64::from(input) / u64::from(output)) as u32
            };
            (
                map(x, source.width(), width),
                map(x + 1, source.width(), width),
                map(y, source.height(), height),
                map(y + 1, source.height(), height),
            )
        };
        let mut sums = [0_u64; 4];
        let mut count = 0;
        for sy in top..bottom {
            for sx in left..right {
                let rgba = source.get_pixel(sx, sy).0;
                let alpha = u64::from(rgba[3]);
                for channel in 0..3 {
                    sums[channel] += u64::from(rgba[channel]) * alpha;
                }
                sums[3] += alpha;
                count += 1;
            }
        }
        let alpha = sums[3];
        // Small blocks fit even rounded premultiplied sums in u32; large reductions need u64.
        let divide = |numerator: u64, denominator: u64| {
            if count <= u64::from(u16::MAX) {
                u64::from(numerator as u32 / denominator as u32)
            } else {
                numerator / denominator
            }
        };
        let output_alpha = divide(alpha + count / 2, count);
        *pixel = image::Rgba([
            if output_alpha == 0 {
                0
            } else {
                divide(sums[0] + alpha / 2, alpha) as u8
            },
            if output_alpha == 0 {
                0
            } else {
                divide(sums[1] + alpha / 2, alpha) as u8
            },
            if output_alpha == 0 {
                0
            } else {
                divide(sums[2] + alpha / 2, alpha) as u8
            },
            output_alpha as u8,
        ]);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_dimensions_validate_each_axis_and_average_complete_source_boxes() {
        let source = image::RgbaImage::from_fn(5, 3, |x, y| {
            image::Rgba([(x * 20) as u8, (y * 40) as u8, 100, 255])
        });
        let resized = downscale_export(&source, ExportSize::Exact(2, 1)).unwrap();
        assert_eq!(resized.dimensions(), (2, 1));
        assert_eq!(resized.get_pixel(0, 0).0, [10, 40, 100, 255]);
        assert_eq!(resized.get_pixel(1, 0).0, [60, 40, 100, 255]);
        for (width, height) in [(0, 1), (1, 0), (6, 1), (1, 4), (u32::MAX, 1), (1, u32::MAX)] {
            assert!(downscale_export(&source, ExportSize::Exact(width, height)).is_err());
        }
        assert!(ExportSize::Exact(100_000, 100_000).validate().is_err());
        for (width, height) in [(5, 3), (5, 1), (1, 3), (1, 1)] {
            let size = ExportSize::Exact(width, height);
            assert_eq!(size.dimensions(5, 3).unwrap(), (width, height));
            assert_eq!(downscale_export(&source, size).unwrap().dimensions(), (width, height));
        }
        assert_eq!(downscale_export(&source, ExportSize::Exact(5, 3)).unwrap(), source);
        let mut alpha = image::RgbaImage::from_pixel(5, 3, image::Rgba([0, 0, 255, 0]));
        alpha.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        let resized = downscale_export(&alpha, ExportSize::Exact(2, 1)).unwrap();
        assert_eq!(resized.get_pixel(0, 0).0, [255, 0, 0, 43]);
        assert_eq!(resized.get_pixel(1, 0).0, [0, 0, 0, 0]);
    }

    #[test]
    fn custom_height_matches_transposed_width_for_pixels_alpha_and_skinny_images() {
        assert_eq!(ExportSize::Height(360).dimensions(1920, 1080).unwrap(), (640, 360));
        assert_eq!(ExportSize::Height(2).dimensions(5, 3).unwrap(), (4, 2));
        for invalid in [0, 4, u32::MAX] {
            assert!(ExportSize::Height(invalid).dimensions(5, 3).is_err());
        }
        for (width, height) in [(5, 3), (1, 9), (9, 1), (1, 1), (1, 100_000)] {
            let source = image::RgbaImage::from_fn(width, height, |x, y| {
                image::Rgba([(x % 256) as u8, (y % 256) as u8, 100, if y % 2 == 0 { 255 } else { 0 }])
            });
            for target in [1, height.div_ceil(2), height] {
                let resized = downscale_export(&source, ExportSize::Height(target)).unwrap();
                let transposed = image::RgbaImage::from_fn(height, width, |x, y| *source.get_pixel(y, x));
                let expected = downscale_export(&transposed, ExportSize::Width(target)).unwrap();
                assert_eq!(resized.dimensions(), (expected.height(), expected.width()));
                for (x, y, pixel) in resized.enumerate_pixels() {
                    assert_eq!(pixel, expected.get_pixel(y, x));
                }
            }
        }
    }

    #[test]
    fn custom_width_preserves_bounds_coverage_and_uses_wide_accumulators() {
        assert_eq!(
            ExportSize::Width(640).dimensions(1920, 1080).unwrap(),
            (640, 360)
        );
        assert_eq!(ExportSize::Width(2).dimensions(3, 5).unwrap(), (2, 4));
        assert!(ExportSize::Width(4).dimensions(3, 5).is_err());
        assert!(ExportSize::Width(0).dimensions(3, 5).is_err());
        assert!(ExportSize::Width(u32::MAX).dimensions(3, 5).is_err());
        let source = image::RgbaImage::from_fn(5, 3, |x, y| {
            image::Rgba([(x * 20) as u8, (y * 40) as u8, 100, 255])
        });
        let resized = downscale_export(&source, ExportSize::Width(2)).unwrap();
        assert_eq!(resized.dimensions(), (2, 2));
        assert_eq!(resized.get_pixel(0, 0).0, [10, 0, 100, 255]);
        assert_eq!(resized.get_pixel(1, 1).0, [60, 60, 100, 255]);
        assert_eq!(
            downscale_export(&source, ExportSize::Width(5)).unwrap(),
            source
        );
        let large = image::RgbaImage::from_pixel(100_000, 1, image::Rgba([255, 255, 255, 255]));
        assert_eq!(
            downscale_export(&large, ExportSize::Width(1))
                .unwrap()
                .get_pixel(0, 0)
                .0,
            [255, 255, 255, 255]
        );
        let boundary = image::RgbaImage::from_pixel(65_535, 1, image::Rgba([255, 255, 255, 255]));
        assert_eq!(
            downscale_export(&boundary, ExportSize::Width(1))
                .unwrap()
                .get_pixel(0, 0)
                .0,
            [255, 255, 255, 255]
        );
        let mut transparent = image::RgbaImage::from_pixel(5, 1, image::Rgba([0, 0, 255, 0]));
        transparent.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        assert_eq!(
            downscale_export(&transparent, ExportSize::Width(1))
                .unwrap()
                .get_pixel(0, 0)
                .0,
            [255, 0, 0, 51]
        );
    }

    #[test]
    fn scaled_dimensions_box_values_and_alpha_are_bounded_and_correct() {
        let source = image::RgbaImage::from_fn(4, 4, |x, y| {
            image::Rgba([(x * 40) as u8, (y * 60) as u8, 100, 255])
        });
        let half = downscale_export(&source, ExportSize::Half).unwrap();
        assert_eq!(half.dimensions(), (2, 2));
        assert_eq!(half.get_pixel(0, 0).0, [20, 30, 100, 255]);
        assert_eq!(half.get_pixel(1, 1).0, [100, 150, 100, 255]);
        assert_eq!(
            downscale_export(&source, ExportSize::Quarter)
                .unwrap()
                .get_pixel(0, 0)
                .0,
            [60, 90, 100, 255]
        );
        let mut transparent = image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 255, 0]));
        transparent.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        assert_eq!(
            downscale_export(&transparent, ExportSize::Half)
                .unwrap()
                .get_pixel(0, 0)
                .0,
            [255, 0, 0, 64]
        );
        assert_eq!(
            downscale_export(&transparent, ExportSize::Original).unwrap(),
            transparent
        );
        transparent.put_pixel(0, 0, image::Rgba([255, 0, 0, 1]));
        assert_eq!(
            downscale_export(&transparent, ExportSize::Half)
                .unwrap()
                .get_pixel(0, 0)
                .0,
            [0, 0, 0, 0]
        );
        for (width, height) in [(1, 1), (1, 9), (9, 1), (3, 5)] {
            let source =
                image::RgbaImage::from_pixel(width, height, image::Rgba([12, 34, 56, 255]));
            for (percent, factor) in [(ExportSize::Half, 2), (ExportSize::Quarter, 4)] {
                let scaled = downscale_export(&source, percent).unwrap();
                assert_eq!(
                    scaled.dimensions(),
                    (width.div_ceil(factor), height.div_ceil(factor))
                );
                assert!(scaled.pixels().all(|pixel| pixel.0 == [12, 34, 56, 255]));
            }
        }
        for percent in [ExportSize::Width(0), ExportSize::Width(u32::MAX)] {
            assert!(downscale_export(&source, percent).is_err());
        }
        assert!(downscale_export(&image::RgbaImage::new(0, 1), ExportSize::Half).is_err());
    }
}
