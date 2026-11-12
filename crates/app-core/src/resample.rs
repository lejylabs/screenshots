use crate::{CaptureError, validate_image_size};

pub(crate) fn validate_export_scale(percent: u8) -> Result<(), CaptureError> {
    if !matches!(percent, 25 | 50 | 100) {
        return Err(CaptureError::Backend(
            "Choose Original, 50% or 25% export size".into(),
        ));
    }
    Ok(())
}

/// Downscale finalized RGBA pixels without a full-size float intermediate.
pub(crate) fn downscale_export(
    source: &image::RgbaImage,
    percent: u8,
) -> Result<image::RgbaImage, CaptureError> {
    validate_export_scale(percent)?;
    validate_image_size(source.width(), source.height())?;
    if percent == 100 {
        return Ok(source.clone());
    }
    let factor = 100 / u32::from(percent);
    let width = source.width().div_ceil(factor);
    let height = source.height().div_ceil(factor);
    validate_image_size(width, height)?;
    let mut output = image::RgbaImage::new(width, height);
    for (x, y, pixel) in output.enumerate_pixels_mut() {
        let left = x * factor;
        let top = y * factor;
        let mut sums = [0_u32; 4];
        let mut count = 0;
        for sy in top..(top + factor).min(source.height()) {
            for sx in left..(left + factor).min(source.width()) {
                let rgba = source.get_pixel(sx, sy).0;
                let alpha = u32::from(rgba[3]);
                for channel in 0..3 {
                    sums[channel] += u32::from(rgba[channel]) * alpha;
                }
                sums[3] += alpha;
                count += 1;
            }
        }
        let alpha = sums[3];
        let output_alpha = (alpha + count / 2) / count;
        *pixel = image::Rgba([
            if output_alpha == 0 {
                0
            } else {
                ((sums[0] + alpha / 2) / alpha) as u8
            },
            if output_alpha == 0 {
                0
            } else {
                ((sums[1] + alpha / 2) / alpha) as u8
            },
            if output_alpha == 0 {
                0
            } else {
                ((sums[2] + alpha / 2) / alpha) as u8
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
    fn scaled_dimensions_box_values_and_alpha_are_bounded_and_correct() {
        let source = image::RgbaImage::from_fn(4, 4, |x, y| {
            image::Rgba([(x * 40) as u8, (y * 60) as u8, 100, 255])
        });
        let half = downscale_export(&source, 50).unwrap();
        assert_eq!(half.dimensions(), (2, 2));
        assert_eq!(half.get_pixel(0, 0).0, [20, 30, 100, 255]);
        assert_eq!(half.get_pixel(1, 1).0, [100, 150, 100, 255]);
        assert_eq!(
            downscale_export(&source, 25).unwrap().get_pixel(0, 0).0,
            [60, 90, 100, 255]
        );
        let mut transparent = image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 255, 0]));
        transparent.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        assert_eq!(
            downscale_export(&transparent, 50)
                .unwrap()
                .get_pixel(0, 0)
                .0,
            [255, 0, 0, 64]
        );
        assert_eq!(downscale_export(&transparent, 100).unwrap(), transparent);
        transparent.put_pixel(0, 0, image::Rgba([255, 0, 0, 1]));
        assert_eq!(downscale_export(&transparent, 50).unwrap().get_pixel(0, 0).0, [0, 0, 0, 0]);
        for (width, height) in [(1, 1), (1, 9), (9, 1), (3, 5)] {
            let source =
                image::RgbaImage::from_pixel(width, height, image::Rgba([12, 34, 56, 255]));
            for (percent, factor) in [(50, 2), (25, 4)] {
                let scaled = downscale_export(&source, percent).unwrap();
                assert_eq!(
                    scaled.dimensions(),
                    (width.div_ceil(factor), height.div_ceil(factor))
                );
                assert!(scaled.pixels().all(|pixel| pixel.0 == [12, 34, 56, 255]));
            }
        }
        for percent in [0, 1, 26, 75, 101, 255] {
            assert!(downscale_export(&source, percent).is_err());
        }
        assert!(downscale_export(&image::RgbaImage::new(0, 1), 50).is_err());
    }
}
