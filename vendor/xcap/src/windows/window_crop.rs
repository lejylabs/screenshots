//! Physical GDI window coordinates. No DPI inference, interpolation or clamped crop.
use image::RgbaImage;

pub(super) fn physical_crop(
    raw: [i32; 4],
    visible: [i32; 4],
) -> Result<(u32, u32, u32, u32), &'static str> {
    let width = i64::from(raw[2]) - i64::from(raw[0]);
    let height = i64::from(raw[3]) - i64::from(raw[1]);
    let output_width = i64::from(visible[2]) - i64::from(visible[0]);
    let output_height = i64::from(visible[3]) - i64::from(visible[1]);
    if width <= 0
        || height <= 0
        || width > i64::from(i32::MAX)
        || height > i64::from(i32::MAX)
        || output_width <= 0
        || output_height <= 0
        || visible[0] < raw[0]
        || visible[1] < raw[1]
        || visible[2] > raw[2]
        || visible[3] > raw[3]
    {
        return Err("DWM bounds do not fit the physical window rectangle");
    }
    Ok((
        (i64::from(visible[0]) - i64::from(raw[0])) as u32,
        (i64::from(visible[1]) - i64::from(raw[1])) as u32,
        output_width as u32,
        output_height as u32,
    ))
}

pub(super) fn crop_gdi_window(
    image: RgbaImage,
    raw: [i32; 4],
    visible: [i32; 4],
) -> Result<RgbaImage, &'static str> {
    let (x, y, width, height) = physical_crop(raw, visible)?;
    let expected = (
        (i64::from(raw[2]) - i64::from(raw[0])) as u32,
        (i64::from(raw[3]) - i64::from(raw[1])) as u32,
    );
    if image.dimensions() != expected
        || image.as_raw().len() as u64 != u64::from(expected.0) * u64::from(expected.1) * 4
    {
        return Err("GDI bitmap dimensions disagree with the physical window rectangle");
    }
    let mut result = if x == 0 && y == 0 && (width, height) == expected {
        image
    } else {
        image::imageops::crop_imm(&image, x, y, width, height).to_image()
    };
    // GetDIBits BI_RGB32 defines RGB; its high byte is reserved, not alpha.
    for pixel in result.pixels_mut() {
        pixel[3] = 255;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    #[test]
    fn dwm_resize_borders_are_cropped_once_without_resizing_or_losing_right_bottom_pixels() {
        let source = RgbaImage::from_fn(18, 12, |x, y| {
            Rgba([
                (x * 13 + y * 3) as u8,
                (x * 7 + y * 17) as u8,
                (x * 19 + y * 11) as u8,
                0,
            ])
        });
        let result = crop_gdi_window(source.clone(), [-9, -5, 9, 7], [-7, -4, 6, 5]).unwrap();
        assert_eq!(result.dimensions(), (13, 9));
        for y in 0..9 {
            for x in 0..13 {
                let pixel = source.get_pixel(x + 2, y + 1);
                assert_eq!(
                    *result.get_pixel(x, y),
                    Rgba([pixel[0], pixel[1], pixel[2], 255])
                );
            }
        }
        let native = crop_gdi_window(source.clone(), [0, 0, 18, 12], [0, 0, 18, 12]).unwrap();
        assert_eq!(native.dimensions(), source.dimensions());
        assert_eq!(
            native.get_pixel(17, 11).0[..3],
            source.get_pixel(17, 11).0[..3]
        );
    }
    #[test]
    fn gdi_crop_rejects_incompatible_geometry_instead_of_guessing_scaling_or_clamping() {
        for visible in [
            [-1, 0, 10, 10],
            [0, 0, 11, 10],
            [0, 0, 10, 11],
            [1, 1, 1, 2],
            [3, 2, 2, 1],
        ] {
            assert!(physical_crop([0, 0, 10, 10], visible).is_err());
        }
        assert!(physical_crop([i32::MIN, 0, i32::MAX, 10], [0, 0, 10, 10]).is_err());
        assert!(crop_gdi_window(RgbaImage::new(9, 10), [0, 0, 10, 10], [1, 0, 9, 10]).is_err());
    }
}
